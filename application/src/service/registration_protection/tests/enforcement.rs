//! Preparation tests: real PostgreSQL, fake providers; NOT real-Kratos D15 acceptance.
use super::*;
use crate::service::registration_enforcement::{
    EnforceRegistrationUseCase, RegistrationAttempt, RegistrationVerdict,
};

fn fault(outcome: ProviderOutcome, stage: CheckStage, at: OffsetDateTime) -> ProviderResult {
    ProviderResult::new(
        outcome,
        if outcome == ProviderOutcome::ConfigurationInvalid {
            ReasonCode::InvalidConfiguration
        } else {
            ReasonCode::Timeout
        },
        TransportClass::Timeout,
    )
    .at_stage(stage)
    .observed(at)
}

const FLOW: &str = "89c3e9c6-9a10-4e1c-ae02-a04b53cc4166";
const EMAIL: &str = "registration@example.com";

fn attempt<'a>(proof: Option<&'a str>, token: Option<&'a str>) -> RegistrationAttempt<'a> {
    RegistrationAttempt {
        flow_id: FLOW,
        email: EMAIL,
        email_proof: proof,
        bot_token: token,
    }
}

async fn proof(f: &Fixture) -> String {
    f.set_flow(FLOW, Duration::minutes(10));
    let issued = f
        .module
        .issue_registration_email_challenge(&f.issue_request(EMAIL, FLOW, "source"))
        .await
        .unwrap();
    let IssueOutcome::Issued { challenge_id, .. } = issued else {
        panic!("{issued:?}");
    };
    let (_, code) = f.last_sent();
    let ConfirmOutcome::Confirmed { proof, .. } = f
        .module
        .confirm_registration_email_challenge(&challenge_id, &code)
        .await
        .unwrap()
    else {
        panic!("expected proof");
    };
    proof
}

async fn due(f: &Fixture) {
    let mut connection = f.module.db.connection().await.unwrap();
    let con: &mut PgConnection = &mut connection;
    sqlx::query(
        "UPDATE registration_protection_schedule SET next_run_at = now() - interval '1 second'",
    )
    .execute(con)
    .await
    .unwrap();
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn final_submit_requires_effective_protections_and_rechecks_old_flow_after_closure() {
    let f = Fixture::new().await;
    f.bootstrap_email().await;
    f.bootstrap_bot().await;
    f.set_gate_admin_state(AdminState::Open).await;
    assert_eq!(
        f.module
            .enforce_registration(attempt(None, Some("valid")))
            .await
            .unwrap(),
        RegistrationVerdict::Reject
    );
    let proof = proof(&f).await;
    assert_eq!(
        f.module
            .enforce_registration(attempt(Some(&proof), None))
            .await
            .unwrap(),
        RegistrationVerdict::Reject
    );
    assert_eq!(
        f.module
            .enforce_registration(attempt(Some(&proof), Some("valid")))
            .await
            .unwrap(),
        RegistrationVerdict::Accept
    );
    // Transport retry reuses the verified email fact, not the gate verdict.
    let gate = f.gate().await;
    f.module
        .close_registration_gate(&f.admin(), gate.revision)
        .await
        .unwrap();
    assert_eq!(
        f.module
            .enforce_registration(attempt(Some(&proof), Some("valid")))
            .await
            .unwrap(),
        RegistrationVerdict::Reject
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn bot_only_bad_token_never_downgrades_provider_or_creates_notifications() {
    let f = Fixture::new().await;
    f.bootstrap_bot().await;
    f.set_gate_admin_state(AdminState::Open).await;
    let baseline = f.module.sync_registration_observation().await.unwrap();
    let events = f
        .scalar("SELECT count(*) FROM registration_gate_events")
        .await;
    *f.module.turnstile.verify.lock().unwrap() =
        ProviderResult::attempt_rejected(ReasonCode::TokenRejected, TransportClass::Http2xx);
    for _ in 0..3 {
        assert_eq!(
            f.module
                .enforce_registration(attempt(None, Some("bad")))
                .await
                .unwrap(),
            RegistrationVerdict::Reject
        );
    }
    assert_eq!(f.gate().await.revision, baseline.state.revision);
    assert_eq!(
        f.scalar("SELECT count(*) FROM registration_gate_events")
            .await,
        events
    );
    assert_eq!(
        f.bot_view().await.effective.status,
        ReadinessStatus::Operational
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn provider_outage_degrades_to_email_only_but_both_outages_temporarily_reject() {
    let f = Fixture::new().await;
    f.bootstrap_email().await;
    f.bootstrap_bot().await;
    f.set_gate_admin_state(AdminState::Open).await;
    let proof = proof(&f).await;
    *f.module.turnstile.verify.lock().unwrap() =
        ProviderResult::temporarily_unavailable(ReasonCode::Timeout, TransportClass::Timeout);
    assert_eq!(
        f.module
            .enforce_registration(attempt(Some(&proof), Some("token")))
            .await
            .unwrap(),
        RegistrationVerdict::Accept
    );
    assert_eq!(
        f.module
            .enforce_registration(attempt(Some(&proof), None))
            .await
            .unwrap(),
        RegistrationVerdict::Accept
    );
    *f.module.email.probe.lock().unwrap() = fault(
        ProviderOutcome::TemporarilyUnavailable,
        CheckStage::Connect,
        now(),
    );
    f.module
        .run_registration_protection_probe(Protection::Email)
        .await
        .unwrap();
    assert_eq!(
        f.module
            .enforce_registration(attempt(Some(&proof), None))
            .await
            .unwrap(),
        RegistrationVerdict::TemporarilyUnavailable
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn override_does_not_remove_prepared_protection_and_is_persistent_until_revoked() {
    let f = Fixture::new().await;
    let gate = f.gate().await;
    assert_eq!(
        kind(
            f.module
                .enable_registration_gate_override(&f.admin(), gate.revision, false)
                .await
        ),
        KernelError::Validation
    );
    f.module
        .enable_registration_gate_override(&f.admin(), gate.revision, true)
        .await
        .unwrap();
    assert_eq!(
        f.module
            .enforce_registration(attempt(None, None))
            .await
            .unwrap(),
        RegistrationVerdict::Accept
    );
    f.bootstrap_bot().await;
    assert_eq!(
        f.module
            .enforce_registration(attempt(None, None))
            .await
            .unwrap(),
        RegistrationVerdict::Reject
    );
    assert_eq!(
        f.module
            .enforce_registration(attempt(None, Some("valid")))
            .await
            .unwrap(),
        RegistrationVerdict::Accept
    );
    let gate = f.gate().await;
    assert!(gate.is_override_active());
    f.module
        .revoke_registration_gate_override(&f.admin(), gate.revision)
        .await
        .unwrap();
    assert!(!f.gate().await.is_override_active());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn maintenance_reserves_once_across_concurrent_and_sequential_replicas() {
    let f = Fixture::new().await;
    f.bootstrap_email().await;
    f.bootstrap_bot().await;
    let before = f.module.email.sent.lock().unwrap().len();
    let period = Duration::seconds(30);
    let (left, right) = tokio::join!(
        f.module.run_registration_protection_maintenance(period),
        f.module.run_registration_protection_maintenance(period),
    );
    assert_eq!(usize::from(left.unwrap()) + usize::from(right.unwrap()), 1);
    assert!(!f
        .module
        .run_registration_protection_maintenance(period)
        .await
        .unwrap());
    assert_eq!(f.module.email.sent.lock().unwrap().len(), before + 1);
    assert_eq!(f.last_sent().0, CANARY);
    // A new clone (restart/process equivalent) honors the persisted due time.
    assert!(!f
        .module
        .clone()
        .run_registration_protection_maintenance(period)
        .await
        .unwrap());
    due(&f).await;
    assert!(f
        .module
        .run_registration_protection_maintenance(period)
        .await
        .unwrap());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn periodic_api_evidence_renews_bootstrapped_generations_even_after_business_ttl_expires() {
    let f = Fixture::new().await;
    f.bootstrap_email().await;
    f.bootstrap_bot().await;
    f.set_gate_admin_state(AdminState::Open).await;
    let mut conn = f.module.db.connection().await.unwrap();
    let con: &mut PgConnection = &mut conn;
    sqlx::query("UPDATE registration_protection_readiness SET success_valid_until = now() - interval '25 hours'")
        .execute(con).await.unwrap();
    drop(conn);
    assert_eq!(
        f.module
            .sync_registration_observation()
            .await
            .unwrap()
            .decision,
        RegistrationDecision::TemporaryReject
    );
    f.module
        .run_registration_protection_maintenance(Duration::seconds(30))
        .await
        .unwrap();
    assert!(f
        .module
        .sync_registration_observation()
        .await
        .unwrap()
        .decision
        .accepts());
    assert_eq!(
        f.email_view().await.effective.status,
        ReadinessStatus::Operational
    );
    assert_eq!(
        f.bot_view().await.effective.status,
        ReadinessStatus::Operational
    );
    // Periodic evidence must not bootstrap a different generation.
    f.module.turnstile.snapshot.lock().unwrap().generation = ConfigGeneration::new("new-bot");
    f.module.email.snapshot.lock().unwrap().generation = ConfigGeneration::new("new-email");
    due(&f).await;
    f.module
        .run_registration_protection_maintenance(Duration::seconds(30))
        .await
        .unwrap();
    assert_eq!(
        f.bot_view().await.effective.status,
        ReadinessStatus::Unverified
    );
    assert_eq!(
        f.email_view().await.effective.status,
        ReadinessStatus::Unverified
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn periodic_canary_recovers_delivery_fault_without_duplicate_notification_or_auto_unlatching_config_loss(
) {
    let f = Fixture::new().await;
    f.bootstrap_email().await;
    // Disable BOT to make email outage/configuration loss decisive.
    let mut bot = config(Protection::Bot, "disabled");
    bot.enabled = false;
    bot.issue = Some(ReasonCode::Disabled);
    *f.module.turnstile.snapshot.lock().unwrap() = bot;
    *f.module.turnstile.probe.lock().unwrap() =
        ProviderResult::configuration_invalid(ReasonCode::Disabled, TransportClass::None);
    f.set_gate_admin_state(AdminState::Open).await;
    *f.module.email.canary_send.lock().unwrap() = fault(
        ProviderOutcome::TemporarilyUnavailable,
        CheckStage::Data,
        now(),
    );
    f.module
        .run_registration_protection_maintenance(Duration::seconds(30))
        .await
        .unwrap();
    assert_eq!(
        f.module
            .sync_registration_observation()
            .await
            .unwrap()
            .decision,
        RegistrationDecision::TemporaryReject
    );
    let count = f
        .scalar("SELECT count(*) FROM registration_gate_events")
        .await;
    due(&f).await;
    f.module
        .run_registration_protection_maintenance(Duration::seconds(30))
        .await
        .unwrap();
    assert_eq!(
        f.scalar("SELECT count(*) FROM registration_gate_events")
            .await,
        count
    );
    *f.module.email.canary_send.lock().unwrap() =
        accepted().at_stage(CheckStage::Data).observed(now());
    due(&f).await;
    f.module
        .run_registration_protection_maintenance(Duration::seconds(30))
        .await
        .unwrap();
    assert!(f
        .module
        .sync_registration_observation()
        .await
        .unwrap()
        .decision
        .accepts());
    *f.module.email.canary_send.lock().unwrap() = fault(
        ProviderOutcome::ConfigurationInvalid,
        CheckStage::Data,
        now(),
    );
    due(&f).await;
    f.module
        .run_registration_protection_maintenance(Duration::seconds(30))
        .await
        .unwrap();
    assert!(f.gate().await.config_lost_latch);
    *f.module.email.canary_send.lock().unwrap() =
        accepted().at_stage(CheckStage::Data).observed(now());
    due(&f).await;
    f.module
        .run_registration_protection_maintenance(Duration::seconds(30))
        .await
        .unwrap();
    assert_eq!(
        f.module
            .sync_registration_observation()
            .await
            .unwrap()
            .decision,
        RegistrationDecision::Stopped
    );
    let gate = f.gate().await;
    f.module
        .open_registration_gate(&f.admin(), gate.revision)
        .await
        .unwrap();
    assert!(f
        .module
        .sync_registration_observation()
        .await
        .unwrap()
        .decision
        .accepts());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn maintenance_interval_must_leave_freshness_margin() {
    let f = Fixture::new().await;
    for period in [Duration::ZERO, Duration::minutes(5), Duration::hours(1)] {
        assert_eq!(
            kind(
                f.module
                    .run_registration_protection_maintenance(period)
                    .await
            ),
            KernelError::Validation
        );
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn aborted_cycle_rolls_back_due_reservation_and_releases_the_database_lock() {
    let f = Fixture::new().await;
    let module = f.module.clone();
    let (ready, received) = tokio::sync::oneshot::channel();
    let owner = tokio::spawn(async move {
        let db = module.clone();
        module
            .transaction_manager()
            .transaction(move |executor| {
                Box::pin(async move {
                    assert!(
                        db.registration_protection_repository()
                            .begin_maintenance_cycle(executor, now(), Duration::seconds(30))
                            .await?
                    );
                    let _ = ready.send(());
                    std::future::pending::<()>().await;
                    Ok::<_, Report<KernelError>>(())
                })
            })
            .await
    });
    received.await.unwrap();
    // While the owner is alive, another replica cannot reserve.
    assert!(!f
        .module
        .run_registration_protection_maintenance(Duration::seconds(30))
        .await
        .unwrap());
    owner.abort();
    assert!(owner.await.unwrap_err().is_cancelled());
    // Rolling back a dropped SQLx transaction is asynchronous; the next pooled
    // acquisition drains rollback before reuse.
    assert!(f
        .module
        .run_registration_protection_maintenance(Duration::seconds(30))
        .await
        .unwrap());
}
