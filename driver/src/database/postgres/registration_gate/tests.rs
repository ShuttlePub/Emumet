//! 実 DB (DATABASE_URL) テスト。`test_with::env` により DATABASE_URL 未設定時は skip されるが、
//! CI (driver は PostgreSQL service 付き、coverage は workspace 全体) では必ず実行される。

use super::PostgresRegistrationGateRepository;
use crate::database::{PostgresConnection, PostgresDatabase};
use kernel::interfaces::database::{DatabaseConnection, TransactionManager};
use kernel::interfaces::repository::{
    apply_gate_command, apply_gate_observation, AuthAccountRepository, AuthHostRepository,
    DependOnAuthAccountRepository, DependOnAuthHostRepository, DependOnRegistrationGateRepository,
    RegistrationGateRepository,
};
use kernel::prelude::entity::{
    AdminState, AuthAccountId, GateAction, GateCommand, GateEventKind, HealthEpisode, ProbeStatus,
    ProtectionObservation, RegistrationDecision, RegistrationGateEventId, RegistrationGateState,
    RegistrationObservation,
};
use kernel::test_utils::{AuthAccountBuilder, AuthHostBuilder};
use kernel::KernelError;
use sqlx::types::time::OffsetDateTime;
use sqlx::PgConnection;

static GATE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const H: ProtectionObservation = ProtectionObservation {
    configured: true,
    probe: ProbeStatus::Healthy,
};
const DOWN: ProtectionObservation = ProtectionObservation {
    configured: true,
    probe: ProbeStatus::Unhealthy,
};
const OFF: ProtectionObservation = ProtectionObservation::UNCONFIGURED;
const ACK: GateCommand = GateCommand::EnableOverride {
    warning_acknowledged: true,
};

fn obs(email: ProtectionObservation, bot: ProtectionObservation) -> RegistrationObservation {
    RegistrationObservation::new(email, bot)
}

struct Fixture {
    db: PostgresDatabase,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    async fn new() -> Self {
        let guard = GATE_TEST_LOCK.lock().await;
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.expect("DATABASE_URL set");
        let mut conn = db.connection().await.unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query(
            "TRUNCATE registration_gate_event_reads, registration_gate_events, \
             registration_gate_audit, registration_gate_state",
        )
        .execute(con)
        .await
        .unwrap();
        Self { db, _guard: guard }
    }

    async fn seed_admin(&self) -> AuthAccountId {
        let mut conn = self.db.connection().await.unwrap();
        let host = AuthHostBuilder::new().build();
        let host_id = host.id().clone();
        self.db
            .auth_host_repository()
            .create(&mut conn, &host)
            .await
            .unwrap();
        let account = AuthAccountBuilder::new().host(host_id).build();
        self.db
            .auth_account_repository()
            .create(&mut conn, &account)
            .await
            .unwrap();
        account.id().clone()
    }

    async fn state(&self) -> RegistrationGateState {
        let mut conn = self.db.connection().await.unwrap();
        self.db
            .registration_gate_repository()
            .load(&mut conn)
            .await
            .unwrap()
    }

    async fn command(
        &self,
        revision: i64,
        command: GateCommand,
        actor: &AuthAccountId,
    ) -> error_stack::Result<RegistrationGateState, KernelError> {
        let actor = actor.clone();
        self.db
            .transaction(move |conn| {
                Box::pin(async move {
                    apply_gate_command(
                        &PostgresRegistrationGateRepository,
                        conn,
                        revision,
                        command,
                        &actor,
                        now(),
                    )
                    .await
                })
            })
            .await
    }

    async fn observe_result(
        &self,
        observation: RegistrationObservation,
    ) -> error_stack::Result<RegistrationGateState, KernelError> {
        self.db
            .transaction(move |conn| {
                Box::pin(async move {
                    apply_gate_observation(
                        &PostgresRegistrationGateRepository,
                        conn,
                        observation,
                        now(),
                    )
                    .await
                })
            })
            .await
    }

    async fn observe(&self, observation: RegistrationObservation) -> RegistrationGateState {
        self.observe_result(observation).await.unwrap()
    }

    async fn count(&self, table: &str) -> i64 {
        let mut conn = self.db.connection().await.unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(con)
            .await
            .unwrap()
    }

    async fn event_kinds(&self, admin: &AuthAccountId) -> Vec<GateEventKind> {
        let mut conn = self.db.connection().await.unwrap();
        let mut kinds: Vec<_> = self
            .db
            .registration_gate_repository()
            .list_notifications(&mut conn, admin, None, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|n| n.event.kind)
            .collect();
        kinds.reverse();
        kinds
    }
}

fn decision(state: &RegistrationGateState) -> RegistrationDecision {
    kernel::prelude::entity::evaluate(state)
}

fn kernel_error(result: error_stack::Result<RegistrationGateState, KernelError>) -> KernelError {
    match result.expect_err("expected error").current_context() {
        KernelError::Concurrency => KernelError::Concurrency,
        KernelError::Rejected => KernelError::Rejected,
        KernelError::Validation => KernelError::Validation,
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn gate_row_is_created_closed_with_revision_zero() {
    let fx = Fixture::new().await;
    let state = fx.state().await;
    assert_eq!(state.admin, AdminState::Closed);
    assert_eq!(state.revision, 0);
    assert!(!state.is_override_active());
    assert_eq!(state.observation, RegistrationObservation::UNCONFIGURED);
    assert_eq!(decision(&state), RegistrationDecision::AdminClosed);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn command_persists_state_audit_and_event_together() {
    let fx = Fixture::new().await;
    let admin = fx.seed_admin().await;

    let state = fx.command(0, ACK, &admin).await.unwrap();
    assert_eq!(state.revision, 1);
    assert_eq!(fx.state().await, state);
    assert_eq!(
        state.override_info.as_ref().unwrap().activated_by,
        admin,
        "override activator is persisted for the always-visible warning"
    );

    let mut conn = fx.db.connection().await.unwrap();
    let audit = fx
        .db
        .registration_gate_repository()
        .list_audit(&mut conn, 10)
        .await
        .unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].action, GateAction::EnableOverride);
    assert_eq!(audit[0].actor, Some(admin.clone()));
    assert!(audit[0].warning_acknowledged);
    assert_eq!(audit[0].before.revision, 0);
    assert_eq!(audit[0].after.revision, 1);
    assert!(audit[0].after.override_active);
    assert_eq!(
        fx.event_kinds(&admin).await,
        vec![GateEventKind::OverrideStarted]
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn failure_after_applying_rolls_back_state_audit_and_events_together() {
    let fx = Fixture::new().await;
    let admin = fx.seed_admin().await;
    let actor = admin.clone();

    let result: error_stack::Result<(), KernelError> = fx
        .db
        .transaction(move |conn| {
            Box::pin(async move {
                apply_gate_command(
                    &PostgresRegistrationGateRepository,
                    conn,
                    0,
                    ACK,
                    &actor,
                    now(),
                )
                .await?;
                Err(error_stack::Report::new(KernelError::Internal))
            })
        })
        .await;
    assert!(result.is_err());

    let state = fx.state().await;
    assert_eq!(state.revision, 0, "state rolled back");
    assert_eq!(fx.count("registration_gate_audit").await, 0);
    assert_eq!(fx.count("registration_gate_events").await, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn event_insert_failure_rolls_back_the_state_update() {
    use kernel::prelude::entity::{
        GateTransitionRecord, RegistrationGateAudit, RegistrationGateAuditId, RegistrationGateEvent,
    };
    let fx = Fixture::new().await;
    let admin = fx.seed_admin().await;
    let now = now();
    let current = fx.state().await;
    let mut next = current.clone();
    next.revision = 1;
    next.admin = AdminState::Open;
    let event = |kind| RegistrationGateEvent {
        id: RegistrationGateEventId::default(),
        kind,
        revision: 1,
        actor: None,
        observation: next.observation,
        created_at: now,
    };
    let record = GateTransitionRecord {
        expected_revision: 0,
        state: next.clone(),
        audit: RegistrationGateAudit {
            id: RegistrationGateAuditId::default(),
            action: GateAction::Open,
            actor: Some(admin),
            warning_acknowledged: false,
            before: current.control_snapshot(),
            after: next.control_snapshot(),
            observation: next.observation,
            created_at: now,
        },
        events: vec![
            event(GateEventKind::OutageStarted),
            event(GateEventKind::OutageStarted),
        ],
    };

    let result = fx
        .db
        .transaction(move |conn| {
            Box::pin(async move {
                PostgresRegistrationGateRepository
                    .save_transition(conn, &record)
                    .await
            })
        })
        .await;

    assert!(
        result.is_err(),
        "(revision, kind) is unique at the DB level"
    );
    assert_eq!(fx.state().await.revision, 0);
    assert_eq!(fx.count("registration_gate_audit").await, 0);
    assert_eq!(fx.count("registration_gate_events").await, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn stale_revision_conflicts_and_persists_nothing() {
    let fx = Fixture::new().await;
    let admin = fx.seed_admin().await;
    fx.command(0, ACK, &admin).await.unwrap();

    let result = fx.command(0, GateCommand::Close, &admin).await;

    assert_eq!(kernel_error(result), KernelError::Concurrency);
    assert_eq!(fx.state().await.revision, 1);
    assert_eq!(fx.count("registration_gate_audit").await, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_commands_with_the_same_revision_have_exactly_one_winner() {
    let fx = Fixture::new().await;
    let admin_a = fx.seed_admin().await;
    let admin_b = fx.seed_admin().await;

    let (a, b) = tokio::join!(fx.command(0, ACK, &admin_a), fx.command(0, ACK, &admin_b));

    let outcomes = [a, b];
    let wins = outcomes.iter().filter(|r| r.is_ok()).count();
    assert_eq!(wins, 1);
    let loser = outcomes.into_iter().find(|r| r.is_err()).unwrap();
    assert_eq!(kernel_error(loser), KernelError::Concurrency);
    assert_eq!(fx.state().await.revision, 1);
    assert_eq!(fx.count("registration_gate_audit").await, 1);
    assert_eq!(fx.count("registration_gate_events").await, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_observation_and_command_serialize_without_lost_updates() {
    let fx = Fixture::new().await;
    let admin = fx.seed_admin().await;
    fx.observe(obs(H, OFF)).await;
    let opened = fx.command(0, GateCommand::Open, &admin).await.unwrap();

    let (observed, closed) = tokio::join!(
        fx.observe_result(obs(DOWN, OFF)),
        fx.command(opened.revision, GateCommand::Close, &admin)
    );
    observed.unwrap();
    closed.ok();

    let state = fx.state().await;
    assert_eq!(state.observation, obs(DOWN, OFF), "observation is not lost");
    assert_eq!(
        fx.count("registration_gate_audit").await,
        state.revision,
        "one audit row per revision"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn one_sided_failure_creates_exactly_one_degraded_event_across_repeated_probes() {
    let fx = Fixture::new().await;
    let admin = fx.seed_admin().await;
    fx.observe(obs(H, H)).await;
    let opened = fx.command(0, GateCommand::Open, &admin).await.unwrap();
    assert_eq!(fx.count("registration_gate_events").await, 0);

    let degraded = fx.observe(obs(DOWN, H)).await;
    assert_eq!(degraded.revision, opened.revision + 1);
    for _ in 0..10 {
        let again = fx.observe(obs(DOWN, H)).await;
        assert_eq!(again.revision, degraded.revision);
    }
    fx.observe(obs(
        ProtectionObservation::new(true, ProbeStatus::Unknown),
        H,
    ))
    .await;

    assert_eq!(
        fx.event_kinds(&admin).await,
        vec![GateEventKind::ProtectionDegraded]
    );
    assert_eq!(fx.count("registration_gate_audit").await, 2);
    assert_eq!(fx.state().await.episode, HealthEpisode::Degraded);
    assert!(decision(&fx.state().await).accepts());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn temporary_outage_rejects_then_recovers_without_manual_resume_and_recurs_as_new_episode() {
    let fx = Fixture::new().await;
    let admin = fx.seed_admin().await;
    fx.observe(obs(H, H)).await;
    fx.command(0, GateCommand::Open, &admin).await.unwrap();

    let down = fx.observe(obs(DOWN, DOWN)).await;
    assert_eq!(decision(&down), RegistrationDecision::TemporaryReject);
    fx.observe(obs(DOWN, DOWN)).await;
    let up = fx.observe(obs(H, DOWN)).await;
    assert!(
        decision(&up).accepts(),
        "auto resume, no admin command issued"
    );
    fx.observe(obs(DOWN, DOWN)).await;
    fx.observe(obs(H, H)).await;

    assert_eq!(
        fx.event_kinds(&admin).await,
        vec![
            GateEventKind::OutageStarted,
            GateEventKind::OutageRecovered,
            GateEventKind::ProtectionDegraded,
            GateEventKind::OutageStarted,
            GateEventKind::OutageRecovered,
        ]
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn config_loss_latches_until_manual_resume_and_resume_is_rejected_when_unmet() {
    let fx = Fixture::new().await;
    let admin = fx.seed_admin().await;
    fx.observe(obs(H, OFF)).await;
    let opened = fx.command(0, GateCommand::Open, &admin).await.unwrap();

    let lost = fx.observe(obs(OFF, OFF)).await;
    assert!(lost.config_lost_latch);
    assert_eq!(decision(&lost), RegistrationDecision::Stopped);
    assert_eq!(lost.revision, opened.revision + 1);

    let rejected = fx.command(lost.revision, GateCommand::Open, &admin).await;
    assert_eq!(kernel_error(rejected), KernelError::Rejected);
    assert_eq!(fx.state().await, lost, "rejected resume persists nothing");

    let recovered = fx.observe(obs(H, H)).await;
    assert_eq!(decision(&recovered), RegistrationDecision::Stopped);
    assert!(
        fx.state().await.config_lost_latch,
        "persisted latch survives recovery"
    );

    let resumed = fx
        .command(recovered.revision, GateCommand::Open, &admin)
        .await
        .unwrap();
    assert!(!resumed.config_lost_latch);
    assert!(decision(&resumed).accepts());
    assert_eq!(
        fx.event_kinds(&admin).await,
        vec![GateEventKind::ProtectionLostStopped]
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn manual_close_survives_recovery_observations() {
    let fx = Fixture::new().await;
    let admin = fx.seed_admin().await;
    fx.observe(obs(H, OFF)).await;
    let opened = fx.command(0, GateCommand::Open, &admin).await.unwrap();
    let closed = fx
        .command(opened.revision, GateCommand::Close, &admin)
        .await
        .unwrap();

    for observation in [obs(DOWN, OFF), obs(H, H), obs(OFF, OFF), obs(H, OFF)] {
        let state = fx.observe(observation).await;
        assert_eq!(decision(&state), RegistrationDecision::AdminClosed);
    }
    assert_eq!(fx.state().await.admin, AdminState::Closed);
    assert_eq!(fx.state().await.revision, closed.revision);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn override_lifecycle_is_persisted_audited_and_notified() {
    let fx = Fixture::new().await;
    let admin = fx.seed_admin().await;

    let denied = fx
        .command(
            0,
            GateCommand::EnableOverride {
                warning_acknowledged: false,
            },
            &admin,
        )
        .await;
    assert_eq!(kernel_error(denied), KernelError::Validation);
    assert_eq!(fx.state().await.revision, 0);

    let enabled = fx.command(0, ACK, &admin).await.unwrap();
    assert!(decision(&enabled).accepts());
    let still = fx.observe(obs(OFF, OFF)).await;
    assert!(still.is_override_active() && !still.config_lost_latch);
    assert_eq!(
        still.revision, enabled.revision,
        "no auto stop during override"
    );

    let revoked = fx
        .command(enabled.revision, GateCommand::RevokeOverride, &admin)
        .await
        .unwrap();
    assert!(!revoked.is_override_active());
    assert_eq!(decision(&revoked), RegistrationDecision::Stopped);
    assert_eq!(revoked.admin, AdminState::Open, "revoke does not close");

    assert_eq!(
        fx.event_kinds(&admin).await,
        vec![
            GateEventKind::OverrideStarted,
            GateEventKind::OverrideRevoked,
            GateEventKind::ProtectionLostStopped,
        ]
    );
    let mut conn = fx.db.connection().await.unwrap();
    let audit = fx
        .db
        .registration_gate_repository()
        .list_audit(&mut conn, 10)
        .await
        .unwrap();
    let actions: Vec<_> = audit.iter().rev().map(|a| a.action).collect();
    assert_eq!(
        actions,
        vec![GateAction::EnableOverride, GateAction::RevokeOverride]
    );
    assert!(audit.iter().all(|a| a.actor == Some(admin.clone())));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn read_state_is_independent_per_admin() {
    let fx = Fixture::new().await;
    let admin_a = fx.seed_admin().await;
    let admin_b = fx.seed_admin().await;
    fx.observe(obs(H, H)).await;
    fx.command(0, GateCommand::Open, &admin_a).await.unwrap();
    fx.observe(obs(DOWN, DOWN)).await;
    fx.observe(obs(H, H)).await;
    let repo = fx.db.registration_gate_repository();
    let mut conn = fx.db.connection().await.unwrap();

    assert_eq!(repo.count_unread(&mut conn, &admin_a).await.unwrap(), 2);
    assert_eq!(repo.count_unread(&mut conn, &admin_b).await.unwrap(), 2);

    let all = repo
        .list_notifications(&mut conn, &admin_a, None, 100)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    let newest = all[0].event.id.clone();
    let marked = repo
        .mark_read(
            &mut conn,
            &admin_a,
            &[newest.clone(), RegistrationGateEventId::new(1)],
            now(),
        )
        .await
        .unwrap();
    assert_eq!(marked, 1, "unknown event ids are ignored");
    let again = repo
        .mark_read(&mut conn, &admin_a, std::slice::from_ref(&newest), now())
        .await
        .unwrap();
    assert_eq!(again, 0, "marking twice does not duplicate");

    assert_eq!(repo.count_unread(&mut conn, &admin_a).await.unwrap(), 1);
    assert_eq!(
        repo.count_unread(&mut conn, &admin_b).await.unwrap(),
        2,
        "admin B is unaffected by admin A reading"
    );
    let a_view = repo
        .list_notifications(&mut conn, &admin_a, None, 100)
        .await
        .unwrap();
    let b_view = repo
        .list_notifications(&mut conn, &admin_b, None, 100)
        .await
        .unwrap();
    assert!(a_view[0].read_at.is_some() && a_view[1].read_at.is_none());
    assert!(b_view.iter().all(|n| n.read_at.is_none()));

    repo.mark_all_read(&mut conn, &admin_b, now())
        .await
        .unwrap();
    assert_eq!(repo.count_unread(&mut conn, &admin_b).await.unwrap(), 0);
    assert_eq!(
        repo.count_unread(&mut conn, &admin_a).await.unwrap(),
        1,
        "admin A is unaffected by admin B reading"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn notifications_are_paged_newest_first() {
    let fx = Fixture::new().await;
    let admin = fx.seed_admin().await;
    fx.observe(obs(H, H)).await;
    fx.command(0, GateCommand::Open, &admin).await.unwrap();
    for _ in 0..2 {
        fx.observe(obs(DOWN, DOWN)).await;
        fx.observe(obs(H, H)).await;
    }
    let repo = fx.db.registration_gate_repository();
    let mut conn: PostgresConnection = fx.db.connection().await.unwrap();

    let first = repo
        .list_notifications(&mut conn, &admin, None, 3)
        .await
        .unwrap();
    assert_eq!(first.len(), 3);
    let second = repo
        .list_notifications(&mut conn, &admin, Some(&first[2].event.id), 3)
        .await
        .unwrap();
    assert_eq!(second.len(), 1);
    assert!(first
        .windows(2)
        .all(|w| w[0].event.id.as_ref() > w[1].event.id.as_ref()));
    assert_eq!(first[0].event.kind, GateEventKind::OutageRecovered);
}

/// PostgreSQL の timestamptz はマイクロ秒精度のため、比較できるようあらかじめ丸める。
fn now() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    now.replace_nanosecond(now.nanosecond() / 1000 * 1000)
        .unwrap()
}
