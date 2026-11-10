//! 実 DB (DATABASE_URL) を使う use case の統合テスト。provider・Kratos・Keto は fake だが、
//! challenge/proof・readiness・E15a の gate 状態は実 PostgreSQL に保存される。
//! `test_with::env` により DATABASE_URL 未設定時は skip される (CI では driver / coverage が実行する)。

use super::*;
use crate::service::registration_gate::{
    ChangeRegistrationGateUseCase, GetRegistrationGateUseCase,
};
use driver::database::PostgresDatabase;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::permission::{
    DependOnPermissionChecker, InstanceRole, PermissionChecker, PermissionReq,
};
use kernel::interfaces::repository::{
    DependOnEmailTransport, DependOnRegistrationFlowDirectory,
    DependOnRegistrationProtectionPolicy, DependOnTurnstileVerifier, RegistrationFlowDirectory,
    RegistrationFlowStatus,
};
use kernel::prelude::entity::{
    AdminState, CheckStage, EmailChallengePolicy, ProbeStatus, ProofRejection,
    ProtectionObservation, ReadinessPolicy, RegistrationGateState,
};
use sqlx::PgConnection;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use time::Duration;

mod enforcement;

static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn config(protection: Protection, generation: &str) -> ProtectionConfigSnapshot {
    ProtectionConfigSnapshot {
        protection,
        provider: ProtectionProvider::for_protection(protection),
        enabled: true,
        issue: None,
        generation: ConfigGeneration::new(generation),
        fingerprint: "fp".to_string(),
    }
}

#[derive(Clone)]
struct FakeTurnstile {
    snapshot: Arc<Mutex<ProtectionConfigSnapshot>>,
    verify: Arc<Mutex<ProviderResult>>,
    probe: Arc<Mutex<ProviderResult>>,
    verify_calls: Arc<AtomicUsize>,
}

impl TurnstileVerifier for FakeTurnstile {
    fn snapshot(&self) -> ProtectionConfigSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    async fn verify(&self, _check: &BotTokenCheck<'_>) -> ProviderResult {
        self.verify_calls.fetch_add(1, Ordering::SeqCst);
        *self.verify.lock().unwrap()
    }

    async fn probe_secret(&self) -> ProviderResult {
        *self.probe.lock().unwrap()
    }
}

#[derive(Clone)]
struct FakeEmail {
    snapshot: Arc<Mutex<ProtectionConfigSnapshot>>,
    canary: Option<String>,
    send: Arc<Mutex<ProviderResult>>,
    canary_send: Arc<Mutex<ProviderResult>>,
    probe: Arc<Mutex<ProviderResult>>,
    sent: Arc<Mutex<Vec<(String, String)>>>,
    purposes: Arc<Mutex<Vec<SendPurpose>>>,
}

impl EmailTransport for FakeEmail {
    fn snapshot(&self) -> ProtectionConfigSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    fn canary_address(&self) -> Option<String> {
        self.canary.clone()
    }

    async fn send_code(&self, to: &str, code: &str, purpose: SendPurpose) -> ProviderResult {
        self.purposes.lock().unwrap().push(purpose);
        let result = match purpose {
            SendPurpose::Registration => *self.send.lock().unwrap(),
            SendPurpose::Canary => *self.canary_send.lock().unwrap(),
        };
        if matches!(
            result.outcome,
            ProviderOutcome::Accepted | ProviderOutcome::BusinessSuccess
        ) {
            self.sent
                .lock()
                .unwrap()
                .push((to.to_string(), code.to_string()));
        }
        result
    }

    async fn probe_connection(&self) -> ProviderResult {
        *self.probe.lock().unwrap()
    }
}

#[derive(Clone, Default)]
struct FakeFlows {
    flows: Arc<Mutex<HashMap<String, RegistrationFlowStatus>>>,
    unreachable: Arc<Mutex<bool>>,
    cookies: Arc<Mutex<Vec<Option<String>>>>,
}

impl RegistrationFlowDirectory for FakeFlows {
    async fn lookup(
        &self,
        flow_id: &str,
        browser_cookie: Option<&str>,
    ) -> error_stack::Result<RegistrationFlowStatus, KernelError> {
        self.cookies
            .lock()
            .unwrap()
            .push(browser_cookie.map(str::to_string));
        if *self.unreachable.lock().unwrap() {
            return Err(Report::new(KernelError::Internal));
        }
        Ok(self
            .flows
            .lock()
            .unwrap()
            .get(flow_id)
            .copied()
            .unwrap_or(RegistrationFlowStatus::NotUsable))
    }
}

#[derive(Clone)]
struct FakePermissionChecker {
    admins: Arc<Vec<AuthAccountId>>,
}

impl PermissionChecker for FakePermissionChecker {
    async fn check(
        &self,
        subject: &AuthAccountId,
        _req: &PermissionReq,
    ) -> error_stack::Result<bool, KernelError> {
        Ok(self.admins.contains(subject))
    }

    async fn list_instance_roles(
        &self,
        _subject: &AuthAccountId,
    ) -> error_stack::Result<Vec<InstanceRole>, KernelError> {
        Ok(Vec::new())
    }
}

#[derive(Clone)]
struct Module {
    db: PostgresDatabase,
    turnstile: FakeTurnstile,
    email: FakeEmail,
    flows: FakeFlows,
    permission_checker: FakePermissionChecker,
    readiness_policy: ReadinessPolicy,
    email_policy: EmailChallengePolicy,
}

kernel::impl_database_delegation!(Module, db, PostgresDatabase);

impl DependOnTurnstileVerifier for Module {
    type TurnstileVerifier = FakeTurnstile;
    fn turnstile_verifier(&self) -> &FakeTurnstile {
        &self.turnstile
    }
}

impl DependOnEmailTransport for Module {
    type EmailTransport = FakeEmail;
    fn email_transport(&self) -> &FakeEmail {
        &self.email
    }
}

impl DependOnRegistrationFlowDirectory for Module {
    type RegistrationFlowDirectory = FakeFlows;
    fn registration_flow_directory(&self) -> &FakeFlows {
        &self.flows
    }
}

impl DependOnPermissionChecker for Module {
    type PermissionChecker = FakePermissionChecker;
    fn permission_checker(&self) -> &FakePermissionChecker {
        &self.permission_checker
    }
}

impl DependOnRegistrationProtectionPolicy for Module {
    fn readiness_policy(&self) -> ReadinessPolicy {
        self.readiness_policy
    }

    fn email_challenge_policy(&self) -> EmailChallengePolicy {
        self.email_policy
    }
}

const ADMIN: i64 = 1;
const OUTSIDER: i64 = 2;
const CANARY: &str = "canary@example.com";

struct Fixture {
    module: Module,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

fn accepted() -> ProviderResult {
    ProviderResult::accepted(TransportClass::SmtpAccepted)
}

/// 実 adapter の接続 probe の成功: MAIL FROM まで検証し、宛先は提示しない。
fn probe_ok() -> ProviderResult {
    accepted().at_stage(CheckStage::MailFrom)
}

fn http_accepted() -> ProviderResult {
    ProviderResult::accepted(TransportClass::Http2xx)
}

fn http_success() -> ProviderResult {
    ProviderResult::business_success(TransportClass::Http2xx)
}

impl Fixture {
    async fn new() -> Self {
        let guard = DB_LOCK.lock().await;
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.expect("DATABASE_URL set");
        let mut conn = db.connection().await.unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query(
            "TRUNCATE registration_protection_readiness, registration_email_challenges, \
             registration_email_send_log, registration_gate_event_reads, \
             registration_gate_events, registration_gate_audit, registration_gate_state, \
             registration_protection_schedule",
        )
        .execute(con)
        .await
        .unwrap();
        drop(conn);
        let module = Module {
            db,
            turnstile: FakeTurnstile {
                snapshot: Arc::new(Mutex::new(config(Protection::Bot, "b1"))),
                verify: Arc::new(Mutex::new(http_success())),
                probe: Arc::new(Mutex::new(http_accepted())),
                verify_calls: Arc::new(AtomicUsize::new(0)),
            },
            email: FakeEmail {
                snapshot: Arc::new(Mutex::new(config(Protection::Email, "e1"))),
                canary: Some(CANARY.to_string()),
                send: Arc::new(Mutex::new(accepted())),
                canary_send: Arc::new(Mutex::new(accepted())),
                probe: Arc::new(Mutex::new(probe_ok())),
                sent: Arc::new(Mutex::new(Vec::new())),
                purposes: Arc::new(Mutex::new(Vec::new())),
            },
            flows: FakeFlows::default(),
            permission_checker: FakePermissionChecker {
                admins: Arc::new(vec![AuthAccountId::new(ADMIN)]),
            },
            readiness_policy: ReadinessPolicy {
                success_ttl: Duration::hours(1),
                probe_max_age: Duration::minutes(10),
            },
            email_policy: EmailChallengePolicy::default(),
        };
        Self {
            module,
            _guard: guard,
        }
    }

    fn admin(&self) -> AuthAccountId {
        AuthAccountId::new(ADMIN)
    }

    fn set_email_snapshot(&self, snapshot: ProtectionConfigSnapshot) {
        *self.module.email.snapshot.lock().unwrap() = snapshot;
    }

    fn set_flow(&self, flow: &str, expires_in: Duration) {
        self.module.flows.flows.lock().unwrap().insert(
            flow.to_string(),
            RegistrationFlowStatus::Active {
                expires_at: now() + expires_in,
            },
        );
    }

    async fn gate(&self) -> RegistrationGateState {
        let mut conn = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        self.module
            .registration_gate_repository()
            .load(&mut conn)
            .await
            .unwrap()
    }

    async fn set_gate_admin_state(&self, state: AdminState) {
        self.gate().await;
        let mut conn = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query("UPDATE registration_gate_state SET admin_state = $1")
            .bind(match state {
                AdminState::Open => "open",
                AdminState::Closed => "closed",
            })
            .execute(con)
            .await
            .unwrap();
    }

    async fn views(&self) -> [ProtectionReadinessView; 2] {
        self.module
            .list_registration_protections(&self.admin())
            .await
            .unwrap()
    }

    async fn email_view(&self) -> ProtectionReadinessView {
        self.views().await[0].clone()
    }

    async fn bot_view(&self) -> ProtectionReadinessView {
        self.views().await[1].clone()
    }

    async fn scalar(&self, sql: &str) -> i64 {
        let mut conn = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query_scalar::<_, i64>(sql)
            .fetch_one(con)
            .await
            .unwrap()
    }

    /// 管理者の canary roundtrip でメールを operational にする。
    async fn bootstrap_email(&self) {
        let started = self
            .module
            .verify_registration_protection(
                &self.admin(),
                Protection::Email,
                AdminVerifyInput::default(),
            )
            .await
            .unwrap();
        let VerifyDisposition::PendingConfirmation { challenge_id, .. } = started.disposition
        else {
            panic!("expected a pending canary challenge: {started:?}");
        };
        let (to, code) = self.last_sent();
        assert_eq!(to, CANARY);
        let confirmed = self
            .module
            .verify_registration_protection(
                &self.admin(),
                Protection::Email,
                AdminVerifyInput {
                    challenge_id: Some(challenge_id),
                    code: Some(code),
                    ..AdminVerifyInput::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(confirmed.result.outcome, ProviderOutcome::BusinessSuccess);
        assert_eq!(
            confirmed.readiness.effective.status,
            ReadinessStatus::Operational
        );
    }

    async fn bootstrap_bot(&self) {
        let report = self
            .module
            .verify_registration_protection(
                &self.admin(),
                Protection::Bot,
                AdminVerifyInput {
                    token: Some("widget-token".to_string()),
                    ..AdminVerifyInput::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            report.readiness.effective.status,
            ReadinessStatus::Operational
        );
    }

    fn issue_request<'a>(
        &self,
        email: &'a str,
        flow: &'a str,
        source: &'a str,
    ) -> IssueRequest<'a> {
        IssueRequest {
            email,
            flow_cookie: None,
            flow_id: flow,
            source,
        }
    }

    fn last_sent(&self) -> (String, String) {
        self.module
            .email
            .sent
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    }

    fn sent_count(&self) -> usize {
        self.module.email.sent.lock().unwrap().len()
    }

    fn reset_sent(&self) {
        self.module.email.sent.lock().unwrap().clear();
    }

    async fn ready_for_issuing(&self) {
        self.bootstrap_bot().await;
        self.bootstrap_email().await;
        self.set_gate_admin_state(AdminState::Open).await;
        self.set_flow("flow-1", Duration::hours(1));
        self.reset_sent();
    }
}

fn kind<T: std::fmt::Debug>(result: error_stack::Result<T, KernelError>) -> KernelError {
    match result.expect_err("expected an error").current_context() {
        KernelError::PermissionDenied => KernelError::PermissionDenied,
        KernelError::Validation => KernelError::Validation,
        other => panic!("unexpected error {other:?}"),
    }
}

// ---- 管理者 API・bootstrap ---------------------------------------------------------------------

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn admin_use_cases_reject_non_admins_before_any_provider_call() {
    let fx = Fixture::new().await;
    let outsider = AuthAccountId::new(OUTSIDER);
    assert_eq!(
        kind(fx.module.list_registration_protections(&outsider).await),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind(
            fx.module
                .verify_registration_protection(
                    &outsider,
                    Protection::Bot,
                    AdminVerifyInput {
                        token: Some("t".into()),
                        ..AdminVerifyInput::default()
                    },
                )
                .await
        ),
        KernelError::PermissionDenied
    );
    assert_eq!(fx.module.turnstile.verify_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fx.sent_count(), 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn nothing_is_ready_before_a_real_verification() {
    let fx = Fixture::new().await;
    for view in fx.views().await {
        assert_eq!(view.effective.status, ReadinessStatus::Unverified);
        assert_eq!(view.effective.reason, ReasonCode::NeverVerified);
        assert!(view.last_check.is_none());
    }
    let gate = fx.module.sync_registration_observation().await.unwrap();
    assert_eq!(
        gate.state.observation.email,
        ProtectionObservation::new(true, ProbeStatus::Unknown)
    );
    assert!(!gate.state.observation.email.is_effective());
    assert!(!gate.state.observation.bot.is_effective());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn bot_becomes_operational_only_through_a_business_success_and_rejections_change_nothing() {
    let fx = Fixture::new().await;
    let probe = fx
        .module
        .verify_registration_protection(&fx.admin(), Protection::Bot, AdminVerifyInput::default())
        .await
        .unwrap();
    assert_eq!(probe.result, http_accepted());
    assert!(probe.recorded);
    assert_eq!(
        probe.readiness.effective.status,
        ReadinessStatus::Unverified
    );
    assert_eq!(
        probe.readiness.effective.reason,
        ReasonCode::NoBusinessSuccess
    );
    assert!(!fx.gate().await.observation.bot.is_effective());

    *fx.module.turnstile.verify.lock().unwrap() =
        ProviderResult::attempt_rejected(ReasonCode::TokenRejected, TransportClass::Http2xx);
    let before = fx.bot_view().await;
    let rejected = fx
        .module
        .verify_registration_protection(
            &fx.admin(),
            Protection::Bot,
            AdminVerifyInput {
                token: Some("bad".into()),
                ..AdminVerifyInput::default()
            },
        )
        .await
        .unwrap();
    assert!(!rejected.recorded);
    assert_eq!(rejected.result.outcome, ProviderOutcome::AttemptRejected);
    assert_eq!(fx.bot_view().await, before);

    *fx.module.turnstile.verify.lock().unwrap() = http_success();
    fx.bootstrap_bot().await;
    let view = fx.bot_view().await;
    let last = view.last_check.unwrap();
    assert_eq!(last.outcome, ProviderOutcome::BusinessSuccess);
    assert_eq!(last.transport, TransportClass::Http2xx);
    assert!(last.generation_matches);
    assert!(view.effective.valid_until.is_some());
    assert!(fx.gate().await.observation.bot.is_effective());

    let calls = fx.module.turnstile.verify_calls.load(Ordering::SeqCst);
    assert_eq!(
        kind(
            fx.module
                .verify_registration_protection(
                    &fx.admin(),
                    Protection::Bot,
                    AdminVerifyInput {
                        token: Some("  ".into()),
                        ..AdminVerifyInput::default()
                    },
                )
                .await
        ),
        KernelError::Validation
    );
    assert_eq!(
        fx.module.turnstile.verify_calls.load(Ordering::SeqCst),
        calls
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn email_needs_the_canary_roundtrip_smtp_acceptance_alone_is_not_ready() {
    let fx = Fixture::new().await;
    let started = fx
        .module
        .verify_registration_protection(&fx.admin(), Protection::Email, AdminVerifyInput::default())
        .await
        .unwrap();
    assert!(matches!(
        started.disposition,
        VerifyDisposition::PendingConfirmation { .. }
    ));
    assert_eq!(
        started.readiness.effective.status,
        ReadinessStatus::Unverified
    );
    assert!(!fx.gate().await.observation.email.is_effective());
    let (to, code) = fx.last_sent();
    assert_eq!(to, CANARY);
    let VerifyDisposition::PendingConfirmation { challenge_id, .. } = started.disposition else {
        unreachable!()
    };

    let wrong = fx
        .module
        .verify_registration_protection(
            &fx.admin(),
            Protection::Email,
            AdminVerifyInput {
                challenge_id: Some(challenge_id.clone()),
                code: Some("00000000".into()),
                ..AdminVerifyInput::default()
            },
        )
        .await
        .unwrap();
    assert!(!wrong.recorded);
    assert_eq!(
        wrong.readiness.effective.status,
        ReadinessStatus::Unverified
    );

    assert_eq!(
        fx.module
            .confirm_registration_email_challenge(&challenge_id, &code)
            .await
            .unwrap(),
        ConfirmOutcome::Rejected,
        "a canary challenge is never usable for a registrant"
    );

    let confirmed = fx
        .module
        .verify_registration_protection(
            &fx.admin(),
            Protection::Email,
            AdminVerifyInput {
                challenge_id: Some(challenge_id.clone()),
                code: Some(code.clone()),
                ..AdminVerifyInput::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(confirmed.result.outcome, ProviderOutcome::BusinessSuccess);
    assert_eq!(
        confirmed.readiness.effective.status,
        ReadinessStatus::Operational
    );
    assert!(fx.gate().await.observation.email.is_effective());
    let again = fx
        .module
        .verify_registration_protection(
            &fx.admin(),
            Protection::Email,
            AdminVerifyInput {
                challenge_id: Some(challenge_id),
                code: Some(code),
                ..AdminVerifyInput::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(again.result.outcome, ProviderOutcome::AttemptRejected);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn email_without_a_canary_mailbox_stays_unverified() {
    let mut fx = Fixture::new().await;
    fx.module.email.canary = None;
    let report = fx
        .module
        .verify_registration_protection(&fx.admin(), Protection::Email, AdminVerifyInput::default())
        .await
        .unwrap();
    assert_eq!(report.disposition, VerifyDisposition::CanaryNotConfigured);
    assert_eq!(
        report.readiness.effective.status,
        ReadinessStatus::Unverified
    );
    assert_eq!(fx.sent_count(), 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn invalid_admin_input_combinations_are_validation_errors() {
    let fx = Fixture::new().await;
    for (protection, input) in [
        (
            Protection::Bot,
            AdminVerifyInput {
                code: Some("1".into()),
                ..AdminVerifyInput::default()
            },
        ),
        (
            Protection::Email,
            AdminVerifyInput {
                token: Some("t".into()),
                ..AdminVerifyInput::default()
            },
        ),
        (
            Protection::Email,
            AdminVerifyInput {
                challenge_id: Some("only-id".into()),
                ..AdminVerifyInput::default()
            },
        ),
    ] {
        assert_eq!(
            kind(
                fx.module
                    .verify_registration_protection(&fx.admin(), protection, input)
                    .await
            ),
            KernelError::Validation
        );
    }
}

// ---- 分類と E15a 観測入力 ------------------------------------------------------------------------

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn classifications_map_to_the_fixed_observation_inputs() {
    let fx = Fixture::new().await;
    fx.bootstrap_bot().await;
    fx.bootstrap_email().await;
    let gate = fx.gate().await;
    assert!(gate.observation.email.is_effective() && gate.observation.bot.is_effective());

    *fx.module.turnstile.probe.lock().unwrap() = ProviderResult::temporarily_unavailable(
        ReasonCode::ProviderServerError,
        TransportClass::Http5xx,
    );
    fx.module
        .run_registration_protection_probe(Protection::Bot)
        .await
        .unwrap();
    let state = fx.gate().await;
    assert_eq!(
        state.observation.bot,
        ProtectionObservation::new(true, ProbeStatus::Unhealthy)
    );
    assert!(state.observation.email.is_effective());

    *fx.module.turnstile.probe.lock().unwrap() = http_accepted();
    fx.module
        .run_registration_protection_probe(Protection::Bot)
        .await
        .unwrap();
    assert!(fx.gate().await.observation.bot.is_effective());

    *fx.module.turnstile.probe.lock().unwrap() =
        ProviderResult::configuration_invalid(ReasonCode::InvalidSecret, TransportClass::Http4xx);
    fx.module
        .run_registration_protection_probe(Protection::Bot)
        .await
        .unwrap();
    let state = fx.gate().await;
    assert_eq!(state.observation.bot, ProtectionObservation::UNCONFIGURED);
    assert!(state.observation.email.is_effective());
    assert!(
        !state.config_lost_latch,
        "one protection lost does not latch"
    );
    assert_eq!(
        fx.bot_view().await.effective.status,
        ReadinessStatus::ConfigurationInvalid
    );

    let before = fx.gate().await;
    *fx.module.email.send.lock().unwrap() = ProviderResult::attempt_rejected(
        ReasonCode::RecipientRejected,
        TransportClass::SmtpPermanent,
    );
    fx.set_gate_admin_state(AdminState::Open).await;
    fx.set_flow("flow-r", Duration::hours(1));
    fx.module
        .issue_registration_email_challenge(&fx.issue_request("ghost@example.com", "flow-r", "src"))
        .await
        .unwrap();
    assert_eq!(fx.gate().await.observation, before.observation);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn both_protections_lost_latches_the_gate_and_missing_configuration_is_configuration_invalid()
{
    let fx = Fixture::new().await;
    fx.bootstrap_bot().await;
    fx.bootstrap_email().await;
    fx.set_gate_admin_state(AdminState::Open).await;
    for protection in [Protection::Bot, Protection::Email] {
        let mut snapshot = config(protection, "lost");
        snapshot.enabled = false;
        snapshot.issue = Some(ReasonCode::NotConfigured);
        match protection {
            Protection::Bot => *fx.module.turnstile.snapshot.lock().unwrap() = snapshot,
            Protection::Email => fx.set_email_snapshot(snapshot),
        }
    }
    let gate = fx.module.sync_registration_observation().await.unwrap();
    assert!(gate.state.config_lost_latch);
    assert_eq!(
        gate.state.observation,
        RegistrationObservation::UNCONFIGURED
    );
    for view in fx.views().await {
        assert_eq!(view.effective.status, ReadinessStatus::ConfigurationInvalid);
        assert_eq!(view.effective.reason, ReasonCode::NotConfigured);
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn repeated_probes_do_not_bump_revision_or_notifications() {
    let fx = Fixture::new().await;
    fx.bootstrap_bot().await;
    fx.bootstrap_email().await;
    let revision = fx.gate().await.revision;
    let events_before = fx
        .scalar("SELECT COUNT(*) FROM registration_gate_events")
        .await;
    for _ in 0..5 {
        fx.module
            .run_registration_protection_probe(Protection::Bot)
            .await
            .unwrap();
        fx.module
            .run_registration_protection_probe(Protection::Email)
            .await
            .unwrap();
        fx.module.sync_registration_observation().await.unwrap();
    }
    assert_eq!(fx.gate().await.revision, revision);
    assert_eq!(
        fx.scalar("SELECT COUNT(*) FROM registration_gate_events")
            .await,
        events_before
    );
}

// ---- 鮮度 (注入した時刻) ----------------------------------------------------------------------------

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn freshness_expires_when_probes_stop_and_when_the_success_ages_out_even_with_probes() {
    let fx = Fixture::new().await;
    fx.bootstrap_bot().await;
    fx.bootstrap_email().await;
    let start = now();

    let fresh = fx
        .module
        .registration_protection_readiness_at(start + Duration::minutes(1))
        .await
        .unwrap();
    assert!(fresh
        .iter()
        .all(|v| v.effective.status == ReadinessStatus::Operational));
    let stale_at = start + Duration::minutes(11);
    let stale = fx
        .module
        .registration_protection_readiness_at(stale_at)
        .await
        .unwrap();
    for view in &stale {
        assert_eq!(view.effective.status, ReadinessStatus::Unverified);
        assert_eq!(view.effective.reason, ReasonCode::ProbeStale);
    }
    let observation = fx
        .module
        .registration_observation_at(stale_at)
        .await
        .unwrap();
    assert_eq!(
        observation.email,
        ProtectionObservation::new(true, ProbeStatus::Unknown)
    );
    assert_eq!(
        observation.bot,
        ProtectionObservation::new(true, ProbeStatus::Unknown)
    );
    let gate = fx
        .module
        .sync_registration_observation_at(stale_at)
        .await
        .unwrap();
    assert!(!gate.state.observation.email.is_effective());
    let revision = gate.state.revision;
    fx.module
        .sync_registration_observation_at(stale_at)
        .await
        .unwrap();
    assert_eq!(fx.gate().await.revision, revision);

    let mut at = start;
    let mut last_operational = None;
    for step in 1..=8 {
        at = start + Duration::minutes(9 * step);
        fx.module
            .run_registration_protection_probe_at(Protection::Bot, at)
            .await
            .unwrap();
        let view = fx
            .module
            .registration_protection_readiness_at(at)
            .await
            .unwrap()[1]
            .clone();
        if view.effective.status == ReadinessStatus::Operational {
            last_operational = Some(at);
        } else {
            assert_eq!(
                view.effective.reason,
                ReasonCode::SuccessExpired,
                "at step {step}"
            );
        }
    }
    let last = last_operational.expect("operational while the success is fresh");
    assert!(last < start + Duration::hours(1));
    assert!(at >= start + Duration::hours(1));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_configuration_change_invalidates_readiness_and_runs_back_to_unverified() {
    let fx = Fixture::new().await;
    fx.bootstrap_bot().await;
    fx.bootstrap_email().await;
    fx.set_email_snapshot(config(Protection::Email, "e2"));
    let view = fx.email_view().await;
    assert_eq!(view.effective.status, ReadinessStatus::Unverified);
    assert_eq!(view.effective.reason, ReasonCode::GenerationChanged);
    assert!(!view.last_check.unwrap().generation_matches);
    let gate = fx.module.sync_registration_observation().await.unwrap();
    assert!(!gate.state.observation.email.is_effective());
    assert!(gate.state.observation.bot.is_effective());

    fx.module
        .run_registration_protection_probe(Protection::Email)
        .await
        .unwrap();
    assert_eq!(
        fx.email_view().await.effective.reason,
        ReasonCode::NoBusinessSuccess
    );
    fx.bootstrap_email().await;
    assert_eq!(
        fx.email_view().await.effective.status,
        ReadinessStatus::Operational
    );
}

// ---- 登録前 email challenge / proof ------------------------------------------------------------------

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn challenge_to_proof_to_verification_end_to_end() {
    let fx = Fixture::new().await;
    fx.ready_for_issuing().await;
    let at = now();

    let issued = fx
        .module
        .issue_registration_email_challenge_at(
            &fx.issue_request(" Alice@Example.com ", "flow-1", "203.0.113.1"),
            at,
        )
        .await
        .unwrap();
    let IssueOutcome::Issued {
        challenge_id,
        expires_at,
    } = issued
    else {
        panic!("expected an issued challenge, got {issued:?}");
    };
    let (to, code) = fx.last_sent();
    assert_eq!(to, "alice@example.com");
    assert_eq!(code.len(), 8);
    assert!(code.chars().all(|c| c.is_ascii_digit()));
    assert!(expires_at <= at + Duration::minutes(10) + Duration::seconds(1));
    assert_eq!(
        fx.module
            .verify_registration_email_proof_at("flow-1", "alice@example.com", &challenge_id, at)
            .await
            .unwrap(),
        ProofVerification::Rejected(ProofRejection::Unknown),
        "the challenge id is not a proof"
    );

    let mut conn = fx.module.database_connection().connection().await.unwrap();
    let con: &mut PgConnection = &mut conn;
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT code_hash, email_hash, flow_id FROM registration_email_challenges \
         WHERE purpose = 'registration'",
    )
    .fetch_all(con)
    .await
    .unwrap();
    drop(conn);
    assert_eq!(rows.len(), 1);
    assert_ne!(rows[0].0, code);
    assert!(!rows[0].1.contains("alice"));

    assert_eq!(
        fx.module
            .confirm_registration_email_challenge_at(&challenge_id, "00000000", at)
            .await
            .unwrap(),
        ConfirmOutcome::Rejected
    );
    let ConfirmOutcome::Confirmed {
        proof,
        expires_at: proof_expires,
    } = fx
        .module
        .confirm_registration_email_challenge_at(&challenge_id, &code, at + Duration::minutes(1))
        .await
        .unwrap()
    else {
        panic!("the correct code must confirm");
    };
    assert!(proof_expires <= at + Duration::minutes(16));
    assert_eq!(proof.len(), 64);
    assert_eq!(
        fx.module
            .confirm_registration_email_challenge_at(
                &challenge_id,
                &code,
                at + Duration::minutes(1)
            )
            .await
            .unwrap(),
        ConfirmOutcome::Rejected
    );

    let at = at + Duration::minutes(2);
    let verify = |flow: &'static str, email: &'static str, proof: String, at| {
        let module = fx.module.clone();
        async move {
            module
                .verify_registration_email_proof_at(flow, email, &proof, at)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        verify("flow-other", "alice@example.com", proof.clone(), at).await,
        ProofVerification::Rejected(ProofRejection::FlowMismatch)
    );
    assert_eq!(
        verify("flow-1", "bob@example.com", proof.clone(), at).await,
        ProofVerification::Rejected(ProofRejection::EmailMismatch)
    );
    assert_eq!(
        verify("flow-1", "not an email", proof.clone(), at).await,
        ProofVerification::Rejected(ProofRejection::EmailMismatch)
    );
    assert_eq!(
        verify("flow-1", "alice@example.com", format!("{proof}x"), at).await,
        ProofVerification::Rejected(ProofRejection::Unknown),
        "a tampered proof is unknown"
    );
    assert_eq!(
        verify("flow-1", "ALICE@example.com", proof.clone(), at).await,
        ProofVerification::Verified { replayed: false }
    );
    assert_eq!(
        verify(
            "flow-1",
            "alice@example.com",
            proof.clone(),
            at + Duration::seconds(30)
        )
        .await,
        ProofVerification::Verified { replayed: true }
    );
    assert_eq!(
        verify(
            "flow-1",
            "alice@example.com",
            proof,
            at + Duration::seconds(61)
        )
        .await,
        ProofVerification::Rejected(ProofRejection::AlreadyConsumed)
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn issuing_is_refused_without_sending_unless_the_gate_accepts_and_email_is_operational() {
    let fx = Fixture::new().await;
    fx.set_flow("flow-1", Duration::hours(1));
    assert_eq!(
        fx.module
            .issue_registration_email_challenge(&fx.issue_request("a@example.com", "flow-1", "s"))
            .await
            .unwrap(),
        IssueOutcome::Unavailable
    );
    fx.bootstrap_email().await;
    fx.reset_sent();
    fx.set_gate_admin_state(AdminState::Closed).await;
    assert_eq!(
        fx.module
            .issue_registration_email_challenge(&fx.issue_request("a@example.com", "flow-1", "s"))
            .await
            .unwrap(),
        IssueOutcome::Unavailable
    );
    fx.set_gate_admin_state(AdminState::Open).await;
    let stale = now() + Duration::minutes(30);
    assert_eq!(
        fx.module
            .issue_registration_email_challenge_at(
                &fx.issue_request("a@example.com", "flow-1", "s"),
                stale
            )
            .await
            .unwrap(),
        IssueOutcome::Unavailable
    );
    assert_eq!(fx.sent_count(), 0);
    assert!(matches!(
        fx.module
            .issue_registration_email_challenge(&fx.issue_request("a@example.com", "flow-1", "s"))
            .await
            .unwrap(),
        IssueOutcome::Issued { .. }
    ));
    assert_eq!(fx.sent_count(), 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn issuing_validates_email_and_flow_and_limits_the_rate() {
    let fx = Fixture::new().await;
    fx.ready_for_issuing().await;
    let at = now();
    let issue = |email: &str, flow: &str, source: &str, at| {
        let module = fx.module.clone();
        let (email, flow, source) = (email.to_string(), flow.to_string(), source.to_string());
        async move {
            module
                .issue_registration_email_challenge_at(
                    &IssueRequest {
                        flow_cookie: None,
                        email: &email,
                        flow_id: &flow,
                        source: &source,
                    },
                    at,
                )
                .await
                .unwrap()
        }
    };
    for bad in [
        "",
        "no-at",
        "a@example.com,b@example.com",
        "a b@example.com",
    ] {
        assert_eq!(
            issue(bad, "flow-1", "s", at).await,
            IssueOutcome::InvalidEmail,
            "{bad:?}"
        );
    }
    for bad in ["", "flow with space", "../etc", &"x".repeat(65)] {
        assert_eq!(
            issue("a@example.com", bad, "s", at).await,
            IssueOutcome::InvalidFlow,
            "{bad:?}"
        );
    }
    assert_eq!(
        issue("a@example.com", "unknown-flow", "s", at).await,
        IssueOutcome::InvalidFlow
    );
    fx.set_flow("flow-old", Duration::seconds(-1));
    assert_eq!(
        issue("a@example.com", "flow-old", "s", at).await,
        IssueOutcome::InvalidFlow
    );
    *fx.module.flows.unreachable.lock().unwrap() = true;
    assert_eq!(
        issue("a@example.com", "flow-1", "s", at).await,
        IssueOutcome::Unavailable
    );
    *fx.module.flows.unreachable.lock().unwrap() = false;
    assert_eq!(fx.sent_count(), 0);

    assert!(matches!(
        issue("a@example.com", "flow-1", "s", at).await,
        IssueOutcome::Issued { .. }
    ));
    let limited = issue("A@example.com", "flow-1", "s2", at + Duration::seconds(10)).await;
    assert_eq!(
        limited,
        IssueOutcome::RateLimited {
            retry_after_secs: 50
        }
    );
    assert_eq!(fx.sent_count(), 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn send_failures_never_leave_a_confirmable_challenge_and_do_not_reveal_the_recipient() {
    let fx = Fixture::new().await;
    fx.ready_for_issuing().await;
    let at = now();

    *fx.module.email.send.lock().unwrap() = ProviderResult::attempt_rejected(
        ReasonCode::RecipientRejected,
        TransportClass::SmtpPermanent,
    );
    let outcome = fx
        .module
        .issue_registration_email_challenge_at(
            &fx.issue_request("ghost@example.com", "flow-1", "s"),
            at,
        )
        .await
        .unwrap();
    let IssueOutcome::Issued { challenge_id, .. } = outcome else {
        panic!("a rejected recipient must look like any other issuance: {outcome:?}");
    };
    assert_eq!(
        fx.module
            .confirm_registration_email_challenge_at(&challenge_id, "12345678", at)
            .await
            .unwrap(),
        ConfirmOutcome::Rejected
    );
    assert_eq!(
        fx.email_view().await.effective.status,
        ReadinessStatus::Operational,
        "a recipient rejection never downs the provider"
    );

    *fx.module.email.send.lock().unwrap() = ProviderResult::configuration_invalid(
        ReasonCode::SmtpAuthFailed,
        TransportClass::SmtpAuthRejected,
    );
    let outcome = fx
        .module
        .issue_registration_email_challenge_at(
            &fx.issue_request("b@example.com", "flow-1", "s"),
            at + Duration::seconds(1),
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome, IssueOutcome::Issued { .. }),
        "a provider failure during sending is not distinguishable from any other issuance: {outcome:?}"
    );
    assert_eq!(
        fx.email_view().await.effective.status,
        ReadinessStatus::ConfigurationInvalid
    );
    assert!(!fx.gate().await.observation.email.configured);

    assert_eq!(
        fx.module
            .issue_registration_email_challenge_at(
                &fx.issue_request("c@example.com", "flow-1", "s"),
                at + Duration::seconds(2)
            )
            .await
            .unwrap(),
        IssueOutcome::Unavailable
    );
    assert_eq!(
        fx.scalar(
            "SELECT COUNT(*) FROM registration_email_challenges \
             WHERE status = 'pending' AND purpose = 'registration'"
        )
        .await,
        0
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn smtp_temporary_failure_degrades_email_and_recovers_automatically() {
    let fx = Fixture::new().await;
    fx.ready_for_issuing().await;
    let at = now();
    *fx.module.email.send.lock().unwrap() = ProviderResult::temporarily_unavailable(
        ReasonCode::SmtpTransientFailure,
        TransportClass::SmtpTransient,
    );
    let outcome = fx
        .module
        .issue_registration_email_challenge_at(
            &fx.issue_request("a@example.com", "flow-1", "s"),
            at,
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome, IssueOutcome::Issued { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        fx.module
            .issue_registration_email_challenge_at(
                &fx.issue_request("a2@example.com", "flow-1", "s"),
                at + Duration::seconds(1)
            )
            .await
            .unwrap(),
        IssueOutcome::Unavailable,
        "after the provider is degraded nothing more is sent (independent of the recipient)"
    );
    assert_eq!(
        fx.gate().await.observation.email,
        ProtectionObservation::new(true, ProbeStatus::Unhealthy)
    );
    assert!(fx.gate().await.observation.bot.is_effective());
    *fx.module.email.send.lock().unwrap() = accepted();
    fx.module
        .run_registration_protection_probe(Protection::Email)
        .await
        .unwrap();
    assert!(fx.gate().await.observation.email.is_effective());
    assert!(matches!(
        fx.module
            .issue_registration_email_challenge(&fx.issue_request("z@example.com", "flow-1", "s"))
            .await
            .unwrap(),
        IssueOutcome::Issued { .. }
    ));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn generation_change_invalidates_pending_challenges_and_issued_proofs() {
    let fx = Fixture::new().await;
    fx.ready_for_issuing().await;
    let at = now();
    let IssueOutcome::Issued {
        challenge_id: pending,
        ..
    } = fx
        .module
        .issue_registration_email_challenge_at(
            &fx.issue_request("a@example.com", "flow-1", "s1"),
            at,
        )
        .await
        .unwrap()
    else {
        panic!("issue")
    };
    let pending_code = fx.last_sent().1;
    let IssueOutcome::Issued {
        challenge_id: confirmed,
        ..
    } = fx
        .module
        .issue_registration_email_challenge_at(
            &fx.issue_request("b@example.com", "flow-1", "s2"),
            at,
        )
        .await
        .unwrap()
    else {
        panic!("issue")
    };
    let confirmed_code = fx.last_sent().1;
    let ConfirmOutcome::Confirmed { proof, .. } = fx
        .module
        .confirm_registration_email_challenge_at(&confirmed, &confirmed_code, at)
        .await
        .unwrap()
    else {
        panic!("confirm")
    };

    fx.set_email_snapshot(config(Protection::Email, "e2"));
    assert_eq!(
        fx.module
            .confirm_registration_email_challenge_at(&pending, &pending_code, at)
            .await
            .unwrap(),
        ConfirmOutcome::Rejected
    );
    assert_eq!(
        fx.module
            .verify_registration_email_proof_at("flow-1", "b@example.com", &proof, at)
            .await
            .unwrap(),
        ProofVerification::Rejected(ProofRejection::GenerationMismatch)
    );
    assert_eq!(
        fx.module
            .issue_registration_email_challenge_at(
                &fx.issue_request("c@example.com", "flow-1", "s3"),
                at
            )
            .await
            .unwrap(),
        IssueOutcome::Unavailable
    );
}

// ---- BOT token (hook から呼ぶ内部 use case) ------------------------------------------------------------

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn bot_token_verification_records_provider_outcomes_but_not_user_rejections() {
    let fx = Fixture::new().await;
    assert_eq!(
        fx.module
            .verify_registration_bot_token("token", Some("203.0.113.1"))
            .await
            .unwrap(),
        BotTokenVerdict::Verified
    );
    assert_eq!(
        fx.bot_view().await.effective.status,
        ReadinessStatus::Operational
    );
    assert!(fx.gate().await.observation.bot.is_effective());

    let before = fx.gate().await.observation;
    *fx.module.turnstile.verify.lock().unwrap() =
        ProviderResult::attempt_rejected(ReasonCode::TokenDuplicate, TransportClass::Http2xx);
    assert_eq!(
        fx.module
            .verify_registration_bot_token("replayed", None)
            .await
            .unwrap(),
        BotTokenVerdict::Rejected(ReasonCode::TokenDuplicate)
    );
    assert_eq!(fx.gate().await.observation, before);

    *fx.module.turnstile.verify.lock().unwrap() = ProviderResult::temporarily_unavailable(
        ReasonCode::ProviderRateLimited,
        TransportClass::Http429,
    );
    assert_eq!(
        fx.module
            .verify_registration_bot_token("token", None)
            .await
            .unwrap(),
        BotTokenVerdict::ProviderFailure(
            ProviderOutcome::TemporarilyUnavailable,
            ReasonCode::ProviderRateLimited
        )
    );
    assert_eq!(
        fx.gate().await.observation.bot,
        ProtectionObservation::new(true, ProbeStatus::Unhealthy)
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn the_public_response_sequence_is_identical_whatever_happens_after_the_recipient_is_presented(
) {
    // 宛先を提示した後の結果 (受理・RCPT 550/450/421・DATA 4xx/5xx・RCPT 後の timeout) を、公開応答の列
    // (A への発行の応答と、その後の別宛先 B への発行の応答) から区別できない。結果は実際の SMTP 分類関数で作る。
    use driver::registration_protection::smtp::{classify_smtp_failure, SmtpFailure, SmtpStage};
    let failure = |stage, code: Option<u16>, timeout: bool| {
        classify_smtp_failure(
            SmtpFailure {
                stage,
                code,
                timeout,
                ..SmtpFailure::default()
            },
            SendPurpose::Registration,
        )
    };
    let mut shapes = Vec::new();
    let mut followups = Vec::new();
    for (index, result) in [
        accepted(),
        failure(SmtpStage::Rcpt, Some(550), false),
        failure(SmtpStage::Rcpt, Some(450), false),
        failure(SmtpStage::Rcpt, Some(421), false),
        failure(SmtpStage::Data, Some(452), false),
        failure(SmtpStage::Data, Some(554), false),
        failure(SmtpStage::Data, Some(500), false),
        failure(SmtpStage::Rcpt, None, true),
        failure(SmtpStage::Data, None, true),
        failure(SmtpStage::Rcpt, None, false),
    ]
    .into_iter()
    .enumerate()
    {
        let fx = Fixture::new().await;
        fx.ready_for_issuing().await;
        *fx.module.email.send.lock().unwrap() = result;
        let at = now();
        let outcome = fx
            .module
            .issue_registration_email_challenge_at(
                &fx.issue_request("someone@example.com", "flow-1", "s"),
                at,
            )
            .await
            .unwrap();
        let IssueOutcome::Issued {
            challenge_id,
            expires_at,
        } = outcome
        else {
            panic!("case {index}: {outcome:?}");
        };
        assert_eq!(challenge_id.len(), 32, "case {index}");
        shapes.push(expires_at - at);

        // 後続の別宛先への発行の応答も、先行宛先 A の結果によらず同じ (先行宛先の状態の判別器にならない)。
        *fx.module.email.send.lock().unwrap() = accepted();
        let followup = fx
            .module
            .issue_registration_email_challenge_at(
                &fx.issue_request("other@example.com", "flow-1", "s2"),
                at + Duration::seconds(1),
            )
            .await
            .unwrap();
        let IssueOutcome::Issued { expires_at, .. } = followup else {
            panic!("case {index}: the follow-up issuance must not reveal the earlier recipient: {followup:?}");
        };
        followups.push(expires_at - at);
        assert_eq!(
            fx.email_view().await.effective.status,
            ReadinessStatus::Operational,
            "case {index}: a recipient-dependent result never changes the readiness"
        );
    }
    assert!(shapes.windows(2).all(|pair| pair[0] == pair[1]));
    assert!(followups.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_short_lived_flow_caps_the_challenge_and_the_browser_cookie_is_forwarded() {
    let fx = Fixture::new().await;
    fx.ready_for_issuing().await;
    fx.set_flow("short-flow", Duration::seconds(90));
    let at = now();
    let outcome = fx
        .module
        .issue_registration_email_challenge_at(
            &IssueRequest {
                email: "a@example.com",
                flow_id: "short-flow",
                flow_cookie: Some("csrf_token_x=1"),
                source: "s",
            },
            at,
        )
        .await
        .unwrap();
    let IssueOutcome::Issued { expires_at, .. } = outcome else {
        panic!("{outcome:?}")
    };
    assert!(expires_at <= at + Duration::seconds(90) + Duration::seconds(2));
    assert!(expires_at < at + Duration::minutes(10));
    assert_eq!(
        fx.module
            .flows
            .cookies
            .lock()
            .unwrap()
            .last()
            .cloned()
            .flatten(),
        Some("csrf_token_x=1".to_string())
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_canary_delivery_failure_makes_an_operational_email_unhealthy_while_public_failures_do_not(
) {
    use driver::registration_protection::smtp::{classify_smtp_failure, SmtpFailure, SmtpStage};
    let fx = Fixture::new().await;
    fx.bootstrap_bot().await;
    fx.bootstrap_email().await;
    assert!(fx.gate().await.observation.email.is_effective());
    let failure = SmtpFailure {
        stage: SmtpStage::Data,
        code: Some(452),
        ..SmtpFailure::default()
    };

    // 公開要求で同じ DATA 452 が起きても、観測は変わらない (attempt レベル)。
    fx.set_gate_admin_state(AdminState::Open).await;
    fx.set_flow("flow-1", Duration::hours(1));
    *fx.module.email.send.lock().unwrap() =
        classify_smtp_failure(failure, SendPurpose::Registration);
    fx.module
        .issue_registration_email_challenge(&fx.issue_request("a@example.com", "flow-1", "s"))
        .await
        .unwrap();
    assert_eq!(
        fx.email_view().await.effective.status,
        ReadinessStatus::Operational
    );
    assert!(fx.gate().await.observation.email.is_effective());

    // 管理者 canary (固定宛先) の配送で DATA 452 になれば、provider の一時障害として記録され、
    // operational だったメールは非健全 (Unhealthy) へ遷移する。
    *fx.module.email.canary_send.lock().unwrap() =
        classify_smtp_failure(failure, SendPurpose::Canary);
    let report = fx
        .module
        .verify_registration_protection(&fx.admin(), Protection::Email, AdminVerifyInput::default())
        .await
        .unwrap();
    assert_eq!(
        report.result.outcome,
        ProviderOutcome::TemporarilyUnavailable
    );
    assert!(report.recorded);
    assert_eq!(
        report.readiness.effective.status,
        ReadinessStatus::TemporarilyUnavailable
    );
    assert_eq!(
        fx.gate().await.observation.email,
        ProtectionObservation::new(true, ProbeStatus::Unhealthy)
    );
    assert!(fx.gate().await.observation.bot.is_effective());
    assert_eq!(
        fx.module.email.purposes.lock().unwrap().last().copied(),
        Some(SendPurpose::Canary)
    );

    // canary の配送が設定不正 (RCPT 550 = canary 宛先の不備) なら設定喪失として扱う。
    *fx.module.email.canary_send.lock().unwrap() = classify_smtp_failure(
        SmtpFailure {
            stage: SmtpStage::Rcpt,
            code: Some(550),
            ..SmtpFailure::default()
        },
        SendPurpose::Canary,
    );
    let report = fx
        .module
        .verify_registration_protection(&fx.admin(), Protection::Email, AdminVerifyInput::default())
        .await
        .unwrap();
    assert_eq!(
        report.readiness.effective.status,
        ReadinessStatus::ConfigurationInvalid
    );
    assert!(!fx.gate().await.observation.email.configured);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_delivery_failure_survives_connection_probes_until_a_canary_delivery_succeeds() {
    use driver::registration_protection::smtp::{classify_smtp_failure, SmtpFailure, SmtpStage};
    let fx = Fixture::new().await;
    fx.bootstrap_bot().await;
    fx.bootstrap_email().await;
    assert_eq!(
        fx.email_view().await.effective.status,
        ReadinessStatus::Operational
    );
    let canary_failure = |stage, code| {
        classify_smtp_failure(
            SmtpFailure {
                stage,
                code: Some(code),
                ..SmtpFailure::default()
            },
            SendPurpose::Canary,
        )
    };
    let verify_start = || async {
        fx.module
            .verify_registration_protection(
                &fx.admin(),
                Protection::Email,
                AdminVerifyInput::default(),
            )
            .await
            .unwrap()
    };

    for (failure, expected, observation) in [
        (
            canary_failure(SmtpStage::Data, 452),
            ReadinessStatus::TemporarilyUnavailable,
            ProtectionObservation::new(true, ProbeStatus::Unhealthy),
        ),
        (
            canary_failure(SmtpStage::Rcpt, 550),
            ReadinessStatus::ConfigurationInvalid,
            ProtectionObservation::UNCONFIGURED,
        ),
    ] {
        // operational (または前回の失敗) から、canary の配送失敗で非健全になる。
        *fx.module.email.canary_send.lock().unwrap() = failure;
        let report = verify_start().await;
        assert_eq!(report.readiness.effective.status, expected);
        assert_eq!(fx.gate().await.observation.email, observation);

        // 障害が続いている間は、接続・認証だけの probe が何度成功しても operational へ戻らない
        // (BOT のみへの縮退が解除されない)。
        for _ in 0..3 {
            let probe = fx
                .module
                .run_registration_protection_probe(Protection::Email)
                .await
                .unwrap();
            assert_eq!(probe.outcome, ProviderOutcome::Accepted);
            let view = fx.email_view().await;
            assert_eq!(view.effective.status, expected);
            assert_eq!(view.effective.reason, failure.reason);
            assert_eq!(fx.gate().await.observation.email, observation);
        }

        // 配送範囲の再検証 (canary の配送受理) に成功して初めて解除され、鮮度内の成功で operational に戻る。
        *fx.module.email.canary_send.lock().unwrap() = accepted();
        let recovered = verify_start().await;
        assert_eq!(
            recovered.readiness.effective.status,
            ReadinessStatus::Operational
        );
        assert!(fx.gate().await.observation.email.is_effective());
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_connection_level_failure_is_still_recovered_by_a_connection_probe() {
    let fx = Fixture::new().await;
    fx.bootstrap_bot().await;
    fx.bootstrap_email().await;
    *fx.module.email.probe.lock().unwrap() = ProviderResult::temporarily_unavailable(
        ReasonCode::ConnectFailed,
        TransportClass::ConnectFailed,
    );
    fx.module
        .run_registration_protection_probe(Protection::Email)
        .await
        .unwrap();
    assert_eq!(
        fx.email_view().await.effective.status,
        ReadinessStatus::TemporarilyUnavailable
    );
    *fx.module.email.probe.lock().unwrap() = probe_ok();
    fx.module
        .run_registration_protection_probe(Protection::Email)
        .await
        .unwrap();
    assert_eq!(
        fx.email_view().await.effective.status,
        ReadinessStatus::Operational
    );
    assert!(fx.gate().await.observation.email.is_effective());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_known_configuration_loss_survives_a_later_timeout_and_the_latch_needs_a_manual_resume() {
    use driver::registration_protection::smtp::{classify_smtp_failure, SmtpFailure, SmtpStage};
    let fx = Fixture::new().await;
    fx.bootstrap_bot().await;
    fx.bootstrap_email().await;
    fx.set_gate_admin_state(AdminState::Open).await;
    fx.module.sync_registration_observation().await.unwrap();
    assert!(fx.gate().await.observation.email.is_effective());

    // canary の宛先拒否 (RCPT 550) で配送範囲の configuration_invalid を記録する。
    *fx.module.email.canary_send.lock().unwrap() = classify_smtp_failure(
        SmtpFailure {
            stage: SmtpStage::Rcpt,
            code: Some(550),
            ..SmtpFailure::default()
        },
        SendPurpose::Canary,
    );
    let report = fx
        .module
        .verify_registration_protection(&fx.admin(), Protection::Email, AdminVerifyInput::default())
        .await
        .unwrap();
    assert_eq!(
        report.readiness.effective.status,
        ReadinessStatus::ConfigurationInvalid
    );
    assert_eq!(
        fx.gate().await.observation.email,
        ProtectionObservation::UNCONFIGURED
    );

    // その後の接続 probe の timeout (一時障害) では、既知の configuration_invalid は一時障害へ軽くならない。
    *fx.module.email.probe.lock().unwrap() =
        ProviderResult::temporarily_unavailable(ReasonCode::Timeout, TransportClass::Timeout);
    fx.module
        .run_registration_protection_probe(Protection::Email)
        .await
        .unwrap();
    let view = fx.email_view().await;
    assert_eq!(view.effective.status, ReadinessStatus::ConfigurationInvalid);
    assert_eq!(view.effective.reason, ReasonCode::SmtpPermanentFailure);
    assert_eq!(
        fx.gate().await.observation.email,
        ProtectionObservation::UNCONFIGURED
    );
    assert!(!fx.gate().await.config_lost_latch, "BOT is still healthy");

    // BOT も設定喪失 (無効な secret) になると、両方 configured=false で停止ラッチが成立する。
    *fx.module.turnstile.verify.lock().unwrap() =
        ProviderResult::configuration_invalid(ReasonCode::InvalidSecret, TransportClass::Http4xx);
    fx.module
        .verify_registration_protection(
            &fx.admin(),
            Protection::Bot,
            AdminVerifyInput {
                token: Some("widget-token".to_string()),
                ..AdminVerifyInput::default()
            },
        )
        .await
        .unwrap();
    let latched = fx.gate().await;
    assert!(latched.config_lost_latch);
    assert_eq!(latched.observation, RegistrationObservation::UNCONFIGURED);
    let view = fx.module.get_registration_gate(&fx.admin()).await.unwrap();
    assert_eq!(view.decision, RegistrationDecision::Stopped);

    // 復旧 (BOT の成功・接続 probe の成功・canary の配送成功) だけでは再開しない。
    *fx.module.turnstile.verify.lock().unwrap() = http_success();
    fx.module
        .verify_registration_protection(
            &fx.admin(),
            Protection::Bot,
            AdminVerifyInput {
                token: Some("widget-token".to_string()),
                ..AdminVerifyInput::default()
            },
        )
        .await
        .unwrap();
    *fx.module.email.probe.lock().unwrap() = probe_ok();
    fx.module
        .run_registration_protection_probe(Protection::Email)
        .await
        .unwrap();
    assert_eq!(
        fx.email_view().await.effective.status,
        ReadinessStatus::ConfigurationInvalid,
        "a connection success does not clear the delivery failure"
    );
    *fx.module.email.canary_send.lock().unwrap() = accepted();
    fx.bootstrap_email().await;
    assert_eq!(
        fx.email_view().await.effective.status,
        ReadinessStatus::Operational
    );
    let recovered = fx.gate().await;
    assert!(recovered.observation.email.is_effective());
    assert!(recovered.config_lost_latch, "recovery alone never resumes");
    let view = fx.module.get_registration_gate(&fx.admin()).await.unwrap();
    assert_eq!(view.decision, RegistrationDecision::Stopped);

    // 管理者の手動再開でだけ受付に戻る。
    let resumed = fx
        .module
        .open_registration_gate(&fx.admin(), recovered.revision)
        .await
        .unwrap();
    assert!(resumed.decision.accepts());
    assert!(!resumed.state.config_lost_latch);
}

fn smtp_failure(
    stage: driver::registration_protection::smtp::SmtpStage,
    code: u16,
    purpose: SendPurpose,
) -> ProviderResult {
    use driver::registration_protection::smtp::{classify_smtp_failure, SmtpFailure};
    classify_smtp_failure(
        SmtpFailure {
            stage,
            code: Some(code),
            ..SmtpFailure::default()
        },
        purpose,
    )
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_public_code_confirmation_does_not_clear_a_connection_failure_observed_after_the_send() {
    use driver::registration_protection::smtp::SmtpStage;
    let fx = Fixture::new().await;
    fx.ready_for_issuing().await;
    let IssueOutcome::Issued { challenge_id, .. } = fx
        .module
        .issue_registration_email_challenge_at(
            &fx.issue_request("alice@example.com", "flow-1", "203.0.113.1"),
            now(),
        )
        .await
        .unwrap()
    else {
        panic!("expected an issued challenge");
    };
    let (_, code) = fx.last_sent();

    // 送信の後に、現在の接続・認証の障害が観測される。
    *fx.module.email.probe.lock().unwrap() =
        smtp_failure(SmtpStage::Auth, 535, SendPurpose::Canary);
    fx.module
        .run_registration_protection_probe(Protection::Email)
        .await
        .unwrap();
    assert_eq!(
        fx.email_view().await.effective.status,
        ReadinessStatus::ConfigurationInvalid
    );

    // 障害前の送信に対する code 確認 (過去のメール所有) は、現在の SMTP 認証の復旧の証拠ではない。
    let confirmed = fx
        .module
        .confirm_registration_email_challenge_at(&challenge_id, &code, now())
        .await
        .unwrap();
    assert!(matches!(confirmed, ConfirmOutcome::Confirmed { .. }));
    let view = fx.email_view().await;
    assert_eq!(view.effective.status, ReadinessStatus::ConfigurationInvalid);
    assert_eq!(view.effective.reason, ReasonCode::SmtpAuthFailed);
    assert_eq!(
        fx.gate().await.observation.email,
        ProtectionObservation::UNCONFIGURED
    );

    // SMTP 対話で MAIL FROM まで検証した新しい probe だけが解除する。
    *fx.module.email.probe.lock().unwrap() = probe_ok();
    fx.module
        .run_registration_protection_probe(Protection::Email)
        .await
        .unwrap();
    assert_eq!(
        fx.email_view().await.effective.status,
        ReadinessStatus::Operational
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_older_canary_code_confirmation_does_not_clear_a_later_delivery_failure() {
    use driver::registration_protection::smtp::SmtpStage;
    let fx = Fixture::new().await;
    fx.bootstrap_bot().await;
    let first = fx
        .module
        .verify_registration_protection(&fx.admin(), Protection::Email, AdminVerifyInput::default())
        .await
        .unwrap();
    let VerifyDisposition::PendingConfirmation {
        challenge_id: old_id,
        ..
    } = first.disposition
    else {
        panic!("expected a pending canary challenge");
    };
    let (_, old_code) = fx.last_sent();

    // 新しい canary の送信が宛先拒否で失敗する。古い challenge は新発行では失効しない。
    *fx.module.email.canary_send.lock().unwrap() =
        smtp_failure(SmtpStage::Rcpt, 550, SendPurpose::Canary);
    let failed = fx
        .module
        .verify_registration_protection(&fx.admin(), Protection::Email, AdminVerifyInput::default())
        .await
        .unwrap();
    assert_eq!(
        failed.readiness.effective.status,
        ReadinessStatus::ConfigurationInvalid
    );

    let late = fx
        .module
        .verify_registration_protection(
            &fx.admin(),
            Protection::Email,
            AdminVerifyInput {
                challenge_id: Some(old_id),
                code: Some(old_code),
                ..AdminVerifyInput::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(late.result.outcome, ProviderOutcome::BusinessSuccess);
    assert_eq!(
        late.readiness.effective.status,
        ReadinessStatus::ConfigurationInvalid,
        "a code confirmation is not an SMTP exchange"
    );
    assert_eq!(
        fx.gate().await.observation.email,
        ProtectionObservation::UNCONFIGURED
    );

    // 新しい canary の配送成功 (DATA 受理) と確認で回復する。
    *fx.module.email.canary_send.lock().unwrap() = accepted();
    fx.bootstrap_email().await;
    assert_eq!(
        fx.email_view().await.effective.status,
        ReadinessStatus::Operational
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_mail_from_failure_is_cleared_only_by_a_probe_that_verifies_mail_from() {
    use driver::registration_protection::smtp::SmtpStage;
    let fx = Fixture::new().await;
    fx.ready_for_issuing().await;
    *fx.module.email.send.lock().unwrap() =
        smtp_failure(SmtpStage::MailFrom, 550, SendPurpose::Registration);
    fx.module
        .issue_registration_email_challenge_at(
            &fx.issue_request("alice@example.com", "flow-1", "203.0.113.1"),
            now(),
        )
        .await
        .unwrap();
    let view = fx.email_view().await;
    assert_eq!(view.effective.status, ReadinessStatus::ConfigurationInvalid);
    assert_eq!(view.effective.reason, ReasonCode::SmtpPermanentFailure);

    // 接続・認証までしか検証しない成功では MAIL FROM の障害を解除しない。
    for stage in [CheckStage::Connect, CheckStage::StartTls, CheckStage::Auth] {
        *fx.module.email.probe.lock().unwrap() = accepted().at_stage(stage);
        fx.module
            .run_registration_protection_probe(Protection::Email)
            .await
            .unwrap();
        assert_eq!(
            fx.email_view().await.effective.status,
            ReadinessStatus::ConfigurationInvalid,
            "{stage:?}"
        );
    }
    *fx.module.email.probe.lock().unwrap() = probe_ok();
    fx.module
        .run_registration_protection_probe(Protection::Email)
        .await
        .unwrap();
    assert_eq!(
        fx.email_view().await.effective.status,
        ReadinessStatus::Operational
    );
}
