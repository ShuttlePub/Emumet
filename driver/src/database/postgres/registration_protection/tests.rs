//! 実 DB (DATABASE_URL) テスト。`test_with::env` により DATABASE_URL 未設定時は skip されるが、
//! CI の check-driver (PostgreSQL service 付き) では必ず実行される。
//!
//! readiness 記録・email challenge/proof の原子性 (並行確認・並行消費・並行発行)・期限境界・
//! rate limit・E15a 観測入力への反映 (反復で revision を増やさない) を実 DB で固定する。

use super::{
    PostgresRegistrationEmailChallengeRepository, PostgresRegistrationProtectionRepository,
};
use crate::database::{PostgresDatabase, REGISTRATION_GATE_DB_TEST_LOCK as DB_TEST_LOCK};
use kernel::interfaces::database::{DatabaseConnection, TransactionManager};
use kernel::interfaces::repository::{
    apply_gate_observation, confirm_email_challenge, consume_registration_proof,
    insert_canary_challenge, invalidate_challenge, record_provider_result, reserve_email_challenge,
    ConfirmRejection, ConfirmRequest, ConfirmResult, NewEmailChallenge, ProofVerification,
    RegistrationEmailChallengeRepository, RegistrationProtectionRepository, ReserveOutcome,
};
use kernel::prelude::entity::{
    evaluate_readiness, observation_from_readiness, ChallengePurpose, ChallengeRejection,
    ChallengeStatus, ConfigGeneration, EmailChallengePolicy, ProofBinding, ProofRejection,
    Protection, ProtectionConfigSnapshot, ProtectionProvider, ProviderResult, ReadinessPolicy,
    ReadinessStatus, ReasonCode, SendLimitScope, TransportClass,
};
use kernel::KernelError;
use sqlx::PgConnection;
use std::sync::Arc;
use time::{Duration, OffsetDateTime};

use super::super::registration_gate::PostgresRegistrationGateRepository;

fn t0() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
}

fn policy() -> EmailChallengePolicy {
    EmailChallengePolicy::default()
}

fn readiness_policy() -> ReadinessPolicy {
    ReadinessPolicy {
        success_ttl: Duration::hours(1),
        probe_max_age: Duration::minutes(10),
    }
}

fn snapshot(protection: Protection, generation: &str) -> ProtectionConfigSnapshot {
    ProtectionConfigSnapshot {
        protection,
        provider: ProtectionProvider::for_protection(protection),
        enabled: true,
        issue: None,
        generation: ConfigGeneration::new(generation),
        fingerprint: "fp".to_string(),
    }
}

struct Fixture {
    db: Arc<PostgresDatabase>,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    async fn new() -> Self {
        let guard = DB_TEST_LOCK.lock().await;
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.expect("DATABASE_URL set");
        let mut conn = db.connection().await.unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query(
            "TRUNCATE registration_protection_readiness, registration_email_challenges, \
             registration_email_send_log, registration_gate_event_reads, \
             registration_gate_events, registration_gate_audit, registration_gate_state",
        )
        .execute(con)
        .await
        .unwrap();
        Self {
            db: Arc::new(db),
            _guard: guard,
        }
    }

    async fn record(
        &self,
        config: &ProtectionConfigSnapshot,
        result: ProviderResult,
        at: OffsetDateTime,
    ) -> bool {
        let config = config.clone();
        self.db
            .transaction(move |conn| {
                Box::pin(async move {
                    record_provider_result(
                        &PostgresRegistrationProtectionRepository,
                        conn,
                        &config,
                        &result,
                        at,
                        &readiness_policy(),
                    )
                    .await
                    .map(|record| record.is_some())
                })
            })
            .await
            .unwrap()
    }

    async fn readiness(
        &self,
        protection: Protection,
    ) -> Option<kernel::prelude::entity::ReadinessRecord> {
        let mut conn = self.db.connection().await.unwrap();
        PostgresRegistrationProtectionRepository
            .find(&mut conn, protection)
            .await
            .unwrap()
    }

    async fn reserve(
        &self,
        new: NewEmailChallenge,
        at: OffsetDateTime,
    ) -> error_stack::Result<ReserveOutcome, KernelError> {
        self.db
            .transaction(move |conn| {
                Box::pin(async move {
                    reserve_email_challenge(
                        &PostgresRegistrationEmailChallengeRepository,
                        conn,
                        new,
                        &policy(),
                        at,
                    )
                    .await
                })
            })
            .await
    }

    async fn confirm(
        &self,
        id: &str,
        purpose: ChallengePurpose,
        code_hash: &str,
        proof_hash: &str,
        generation: &str,
        at: OffsetDateTime,
    ) -> ConfirmResult {
        let id = id.to_string();
        let code_hash = code_hash.to_string();
        let proof_hash = proof_hash.to_string();
        let generation = ConfigGeneration::new(generation);
        self.db
            .transaction(move |conn| {
                Box::pin(async move {
                    confirm_email_challenge(
                        &PostgresRegistrationEmailChallengeRepository,
                        conn,
                        &ConfirmRequest {
                            id: &id,
                            expected_purpose: purpose,
                            given_code_hash: &code_hash,
                            new_proof_hash: &proof_hash,
                            current_generation: &generation,
                        },
                        &policy(),
                        at,
                    )
                    .await
                })
            })
            .await
            .unwrap()
    }

    async fn consume(
        &self,
        proof_hash: &str,
        binding: ProofBinding,
        at: OffsetDateTime,
    ) -> ProofVerification {
        let proof_hash = proof_hash.to_string();
        self.db
            .transaction(move |conn| {
                Box::pin(async move {
                    consume_registration_proof(
                        &PostgresRegistrationEmailChallengeRepository,
                        conn,
                        &proof_hash,
                        &binding,
                        &policy(),
                        at,
                    )
                    .await
                })
            })
            .await
            .unwrap()
    }

    async fn count(&self, table: &str) -> i64 {
        let mut conn = self.db.connection().await.unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(con)
            .await
            .unwrap()
    }
}

fn new_challenge(id: &str, email: &str, flow: &str, source: &str) -> NewEmailChallenge {
    NewEmailChallenge {
        id: id.to_string(),
        purpose: ChallengePurpose::Registration,
        flow_id: flow.to_string(),
        email_hash: format!("email-{email}"),
        source_hash: format!("source-{source}"),
        generation: ConfigGeneration::new("g1"),
        code_hash: format!("code-{id}"),
        flow_expires_at: t0() + Duration::hours(1),
    }
}

fn binding(flow: &str, email: &str, generation: &str) -> ProofBinding {
    ProofBinding {
        flow_id: flow.to_string(),
        email_hash: format!("email-{email}"),
        generation: ConfigGeneration::new(generation),
    }
}

fn success() -> ProviderResult {
    ProviderResult::business_success(TransportClass::Http2xx)
}

fn accepted() -> ProviderResult {
    ProviderResult::accepted(TransportClass::Http2xx)
}

// ---- readiness ----------------------------------------------------------------------------

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn readiness_round_trips_and_carries_the_success_only_within_one_generation() {
    let fx = Fixture::new().await;
    let bot = snapshot(Protection::Bot, "g1");
    assert!(fx.readiness(Protection::Bot).await.is_none());

    assert!(fx.record(&bot, success(), t0()).await);
    let stored = fx.readiness(Protection::Bot).await.unwrap();
    assert_eq!(stored.generation, ConfigGeneration::new("g1"));
    assert_eq!(stored.status, ReadinessStatus::Operational);
    assert_eq!(stored.last_success_at, Some(t0()));
    assert_eq!(stored.check_transport, TransportClass::Http2xx);

    // 同一 generation の negative probe は success を引き継ぐ。
    assert!(
        fx.record(&bot, accepted(), t0() + Duration::minutes(5))
            .await
    );
    let carried = fx.readiness(Protection::Bot).await.unwrap();
    assert_eq!(carried.last_success_at, Some(t0()));
    assert_eq!(carried.checked_at, t0() + Duration::minutes(5));

    // 別 generation の検証は旧 success を引き継がない。
    let changed = snapshot(Protection::Bot, "g2");
    assert!(
        fx.record(&changed, accepted(), t0() + Duration::minutes(6))
            .await
    );
    let reset = fx.readiness(Protection::Bot).await.unwrap();
    assert_eq!(reset.generation, ConfigGeneration::new("g2"));
    assert_eq!(reset.last_success_at, None);
    assert_eq!(
        evaluate_readiness(
            &changed,
            Some(&reset),
            t0() + Duration::minutes(6),
            &readiness_policy()
        )
        .status,
        ReadinessStatus::Unverified
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn attempt_rejected_is_not_recorded() {
    let fx = Fixture::new().await;
    let bot = snapshot(Protection::Bot, "g1");
    assert!(fx.record(&bot, success(), t0()).await);
    let before = fx.readiness(Protection::Bot).await.unwrap();
    let rejected =
        ProviderResult::attempt_rejected(ReasonCode::TokenRejected, TransportClass::Http2xx);
    assert!(!fx.record(&bot, rejected, t0() + Duration::minutes(1)).await);
    assert_eq!(fx.readiness(Protection::Bot).await.unwrap(), before);
    assert_eq!(fx.count("registration_protection_readiness").await, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_readiness_writes_never_lose_the_business_success() {
    let fx = Fixture::new().await;
    let bot = snapshot(Protection::Bot, "g1");
    // 初回 (行が存在しない) から並行に success と accepted を書く。
    let mut tasks = Vec::new();
    for index in 0..12i64 {
        let db = fx.db.clone();
        let config = bot.clone();
        let result = if index == 5 { success() } else { accepted() };
        let at = t0() + Duration::seconds(index);
        tasks.push(tokio::spawn(async move {
            db.transaction(move |conn| {
                Box::pin(async move {
                    record_provider_result(
                        &PostgresRegistrationProtectionRepository,
                        conn,
                        &config,
                        &result,
                        at,
                        &readiness_policy(),
                    )
                    .await
                    .map(|_| ())
                })
            })
            .await
            .unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let stored = fx.readiness(Protection::Bot).await.unwrap();
    // 書込順によらず、success を持つ書込みか success を引き継ぐ書込みしかないので、
    // lost update があれば last_success_at が None になる (行ロック・advisory lock で直列化される)。
    assert!(
        stored.last_success_at.is_some(),
        "the business success must never be lost"
    );
    assert_eq!(fx.count("registration_protection_readiness").await, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn repeated_probes_do_not_bump_the_gate_revision_or_events() {
    let fx = Fixture::new().await;
    let email = snapshot(Protection::Email, "e1");
    let bot = snapshot(Protection::Bot, "b1");
    let at = t0();
    assert!(fx.record(&email, success(), at).await);
    assert!(fx.record(&bot, success(), at).await);
    let policy = readiness_policy();

    let observe = |at: OffsetDateTime| {
        let db = fx.db.clone();
        let email = email.clone();
        let bot = bot.clone();
        async move {
            let mut conn = db.connection().await.unwrap();
            let email_record = PostgresRegistrationProtectionRepository
                .find(&mut conn, Protection::Email)
                .await
                .unwrap();
            let bot_record = PostgresRegistrationProtectionRepository
                .find(&mut conn, Protection::Bot)
                .await
                .unwrap();
            drop(conn);
            let observation = observation_from_readiness(
                &evaluate_readiness(&email, email_record.as_ref(), at, &policy),
                &evaluate_readiness(&bot, bot_record.as_ref(), at, &policy),
            );
            db.transaction(move |conn| {
                Box::pin(async move {
                    apply_gate_observation(
                        &PostgresRegistrationGateRepository,
                        conn,
                        observation,
                        at,
                    )
                    .await
                })
            })
            .await
            .unwrap()
        }
    };
    let first = observe(at).await;
    assert!(first.observation.email.is_effective());
    assert!(first.observation.bot.is_effective());
    let revision = first.revision;
    let audits = fx.count("registration_gate_audit").await;
    let events = fx.count("registration_gate_events").await;
    for step in 1..=5 {
        // 同じ評価結果の反復 probe。
        let at = at + Duration::seconds(step);
        assert!(fx.record(&bot, accepted(), at).await);
        let state = observe(at).await;
        assert_eq!(state.revision, revision, "step {step}");
    }
    assert_eq!(fx.count("registration_gate_audit").await, audits);
    assert_eq!(fx.count("registration_gate_events").await, events);

    // 一方 (bot) だけが設定喪失になっても、他方 (email) の観測は巻き込まれず effective のまま。
    let invalid =
        ProviderResult::configuration_invalid(ReasonCode::InvalidSecret, TransportClass::Http4xx);
    let later = at + Duration::minutes(1);
    assert!(fx.record(&bot, invalid, later).await);
    let state = observe(later).await;
    assert!(state.observation.email.is_effective());
    assert!(!state.observation.bot.configured);
    assert!(
        !state.config_lost_latch,
        "one protection lost does not latch"
    );
}

// ---- email challenge: 発行 / rate limit ----------------------------------------------------

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn reserve_stores_only_hashes_and_caps_expiry_at_the_flow() {
    let fx = Fixture::new().await;
    let mut new = new_challenge("c1", "a", "flow-1", "s1");
    new.flow_expires_at = t0() + Duration::minutes(3);
    let outcome = fx.reserve(new, t0()).await.unwrap();
    assert_eq!(
        outcome,
        ReserveOutcome::Reserved {
            expires_at: t0() + Duration::minutes(3)
        }
    );
    let mut conn = fx.db.connection().await.unwrap();
    let stored = PostgresRegistrationEmailChallengeRepository
        .find_for_update(&mut conn, "c1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.code_hash, "code-c1");
    assert_eq!(stored.email_hash, "email-a");
    assert_eq!(stored.status, ChallengeStatus::Pending);
    assert_eq!(stored.attempts, 0);
    assert_eq!(stored.max_attempts, 5);
    assert_eq!(stored.generation, ConfigGeneration::new("g1"));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn rate_limits_per_email_interval_hour_flow_and_source() {
    let fx = Fixture::new().await;
    // 同一 email: 60 秒に 1 通。
    assert!(matches!(
        fx.reserve(new_challenge("a1", "x", "f-a1", "s-a"), t0())
            .await
            .unwrap(),
        ReserveOutcome::Reserved { .. }
    ));
    let limited = fx
        .reserve(
            new_challenge("a2", "x", "f-a2", "s-b"),
            t0() + Duration::seconds(59),
        )
        .await
        .unwrap();
    assert!(matches!(
        limited,
        ReserveOutcome::Limited { scope: SendLimitScope::EmailInterval, retry_after } if retry_after == Duration::seconds(1)
    ));
    // 60 秒ちょうどで再送できる (再送)。
    assert!(matches!(
        fx.reserve(
            new_challenge("a3", "x", "f-a3", "s-c"),
            t0() + Duration::seconds(60)
        )
        .await
        .unwrap(),
        ReserveOutcome::Reserved { .. }
    ));
    // 制限で拒否された発行は challenge を作らない (1 通目と 3 通目だけ)。
    assert_eq!(fx.count("registration_email_challenges").await, 2);

    // 同一 email: 1 時間に 5 通 (1 通目 t0, 2 通目 t0+60s, さらに 3 通 → 計 5)。
    for (index, offset) in [120, 180, 240].into_iter().enumerate() {
        assert!(matches!(
            fx.reserve(
                new_challenge(
                    &format!("h{index}"),
                    "x",
                    &format!("f-h{index}"),
                    &format!("s-h{index}")
                ),
                t0() + Duration::seconds(offset)
            )
            .await
            .unwrap(),
            ReserveOutcome::Reserved { .. }
        ));
    }
    assert!(matches!(
        fx.reserve(
            new_challenge("h-over", "x", "f-over", "s-over"),
            t0() + Duration::seconds(300)
        )
        .await
        .unwrap(),
        ReserveOutcome::Limited {
            scope: SendLimitScope::EmailHourly,
            ..
        }
    ));
    // 1 時間後 (最初の送信が窓の外) には再び送れる。
    assert!(matches!(
        fx.reserve(
            new_challenge("h-after", "x", "f-after", "s-after"),
            t0() + Duration::hours(1) + Duration::seconds(1)
        )
        .await
        .unwrap(),
        ReserveOutcome::Reserved { .. }
    ));

    // 同一 flow: 1 時間に 3 通 (email を変えても flow 単位で制限)。
    for index in 0..3 {
        assert!(matches!(
            fx.reserve(
                new_challenge(
                    &format!("fl{index}"),
                    &format!("fl{index}"),
                    "flow-shared",
                    &format!("s-fl{index}")
                ),
                t0() + Duration::hours(3)
            )
            .await
            .unwrap(),
            ReserveOutcome::Reserved { .. }
        ));
    }
    assert!(matches!(
        fx.reserve(
            new_challenge("fl-over", "fl-over", "flow-shared", "s-fl-over"),
            t0() + Duration::hours(3)
        )
        .await
        .unwrap(),
        ReserveOutcome::Limited {
            scope: SendLimitScope::FlowHourly,
            ..
        }
    ));

    // 同一 source: 1 時間に 20 通。
    for index in 0..20 {
        assert!(matches!(
            fx.reserve(
                new_challenge(
                    &format!("so{index}"),
                    &format!("so{index}"),
                    &format!("f-so{index}"),
                    "src-shared"
                ),
                t0() + Duration::hours(6)
            )
            .await
            .unwrap(),
            ReserveOutcome::Reserved { .. }
        ));
    }
    assert!(matches!(
        fx.reserve(
            new_challenge("so-over", "so-over", "f-so-over", "src-shared"),
            t0() + Duration::hours(6)
        )
        .await
        .unwrap(),
        ReserveOutcome::Limited {
            scope: SendLimitScope::SourceHourly,
            ..
        }
    ));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_issues_cannot_exceed_the_rate_limits() {
    let fx = Fixture::new().await;
    // 同一 email への 10 並行発行: 最小間隔により 1 通だけ通る。
    let mut tasks = Vec::new();
    for index in 0..10 {
        let db = fx.db.clone();
        let new = new_challenge(
            &format!("ce{index}"),
            "same",
            &format!("f-ce{index}"),
            &format!("s-ce{index}"),
        );
        tasks.push(tokio::spawn(async move {
            db.transaction(move |conn| {
                Box::pin(async move {
                    reserve_email_challenge(
                        &PostgresRegistrationEmailChallengeRepository,
                        conn,
                        new,
                        &policy(),
                        t0(),
                    )
                    .await
                })
            })
            .await
            .unwrap()
        }));
    }
    let mut reserved = 0;
    for task in tasks {
        if matches!(task.await.unwrap(), ReserveOutcome::Reserved { .. }) {
            reserved += 1;
        }
    }
    assert_eq!(reserved, 1);
    assert_eq!(fx.count("registration_email_challenges").await, 1);

    // 同一 flow への 10 並行発行 (email は別): flow 上限 3 通だけ通る。
    let mut tasks = Vec::new();
    for index in 0..10 {
        let db = fx.db.clone();
        let new = new_challenge(
            &format!("cf{index}"),
            &format!("cf{index}"),
            "flow-conc",
            &format!("s-cf{index}"),
        );
        tasks.push(tokio::spawn(async move {
            db.transaction(move |conn| {
                Box::pin(async move {
                    reserve_email_challenge(
                        &PostgresRegistrationEmailChallengeRepository,
                        conn,
                        new,
                        &policy(),
                        t0() + Duration::minutes(1),
                    )
                    .await
                })
            })
            .await
            .unwrap()
        }));
    }
    let mut reserved = 0;
    for task in tasks {
        if matches!(task.await.unwrap(), ReserveOutcome::Reserved { .. }) {
            reserved += 1;
        }
    }
    assert_eq!(reserved, 3);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn purge_removes_only_long_expired_rows() {
    let fx = Fixture::new().await;
    fx.reserve(new_challenge("old", "o", "f-old", "s-old"), t0())
        .await
        .unwrap();
    // 2 日後の発行で、1 日以上前に期限切れの challenge と古い送信記録が掃除される。
    let mut new = new_challenge("new", "n", "f-new", "s-new");
    new.flow_expires_at = t0() + Duration::days(2) + Duration::hours(1);
    fx.reserve(new, t0() + Duration::days(2)).await.unwrap();
    let mut conn = fx.db.connection().await.unwrap();
    assert!(PostgresRegistrationEmailChallengeRepository
        .find_for_update(&mut conn, "old")
        .await
        .unwrap()
        .is_none());
    assert!(PostgresRegistrationEmailChallengeRepository
        .find_for_update(&mut conn, "new")
        .await
        .unwrap()
        .is_some());
    drop(conn);
    assert_eq!(fx.count("registration_email_send_log").await, 1);
}

// ---- email challenge: 確認 -------------------------------------------------------------------

async fn issue(fx: &Fixture, id: &str, email: &str, flow: &str) {
    fx.reserve(new_challenge(id, email, flow, &format!("s-{id}")), t0())
        .await
        .unwrap();
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn only_the_correct_code_issues_a_proof_and_wrong_codes_exhaust_the_attempts() {
    let fx = Fixture::new().await;
    issue(&fx, "c1", "a", "flow-1").await;
    let at = t0() + Duration::minutes(1);

    // 送信成功 (= challenge の存在) だけでは proof は発行されない。
    let mut conn = fx.db.connection().await.unwrap();
    let pending = PostgresRegistrationEmailChallengeRepository
        .find_for_update(&mut conn, "c1")
        .await
        .unwrap()
        .unwrap();
    drop(conn);
    assert_eq!(pending.proof_hash, None);

    for attempt in 1..=4 {
        let result = fx
            .confirm(
                "c1",
                ChallengePurpose::Registration,
                "wrong",
                "proof-1",
                "g1",
                at,
            )
            .await;
        assert_eq!(
            result,
            ConfirmResult::Rejected(ConfirmRejection::WrongCode {
                attempts: attempt,
                invalidated: false
            })
        );
    }
    // 5 回目の不一致で失効。以降は正しい code でも拒否される。
    assert_eq!(
        fx.confirm(
            "c1",
            ChallengePurpose::Registration,
            "wrong",
            "proof-1",
            "g1",
            at
        )
        .await,
        ConfirmResult::Rejected(ConfirmRejection::WrongCode {
            attempts: 5,
            invalidated: true
        })
    );
    assert_eq!(
        fx.confirm(
            "c1",
            ChallengePurpose::Registration,
            "code-c1",
            "proof-1",
            "g1",
            at
        )
        .await,
        ConfirmResult::Rejected(ConfirmRejection::Challenge(ChallengeRejection::NotPending))
    );
    let mut conn = fx.db.connection().await.unwrap();
    let stored = PostgresRegistrationEmailChallengeRepository
        .find_for_update(&mut conn, "c1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, ChallengeStatus::Invalidated);
    assert_eq!(stored.proof_hash, None);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn confirmation_boundaries_generation_purpose_and_unknown_ids() {
    let fx = Fixture::new().await;
    issue(&fx, "c1", "a", "flow-1").await;
    let expires = t0() + policy().challenge_ttl;
    // 期限ちょうどは期限切れ、直前は有効。
    assert_eq!(
        fx.confirm(
            "c1",
            ChallengePurpose::Registration,
            "code-c1",
            "p",
            "g1",
            expires
        )
        .await,
        ConfirmResult::Rejected(ConfirmRejection::Challenge(ChallengeRejection::Expired))
    );
    assert!(matches!(
        fx.confirm(
            "c1",
            ChallengePurpose::Registration,
            "code-c1",
            "p-ok",
            "g1",
            expires - Duration::seconds(1)
        )
        .await,
        ConfirmResult::Confirmed {
            purpose: ChallengePurpose::Registration,
            proof_expires_at: Some(_)
        }
    ));

    issue(&fx, "c2", "b", "flow-2").await;
    assert_eq!(
        fx.confirm(
            "c2",
            ChallengePurpose::Registration,
            "code-c2",
            "p2",
            "g-other",
            t0()
        )
        .await,
        ConfirmResult::Rejected(ConfirmRejection::Challenge(
            ChallengeRejection::GenerationChanged
        ))
    );
    assert_eq!(
        fx.confirm("c2", ChallengePurpose::Canary, "code-c2", "p2", "g1", t0())
            .await,
        ConfirmResult::Rejected(ConfirmRejection::PurposeMismatch)
    );
    assert_eq!(
        fx.confirm(
            "missing",
            ChallengePurpose::Registration,
            "x",
            "p",
            "g1",
            t0()
        )
        .await,
        ConfirmResult::Rejected(ConfirmRejection::UnknownChallenge)
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn proof_expiry_is_capped_at_the_flow_expiry() {
    let fx = Fixture::new().await;
    let mut new = new_challenge("c1", "a", "flow-1", "s1");
    new.flow_expires_at = t0() + Duration::minutes(5);
    fx.reserve(new, t0()).await.unwrap();
    let result = fx
        .confirm(
            "c1",
            ChallengePurpose::Registration,
            "code-c1",
            "p1",
            "g1",
            t0() + Duration::minutes(1),
        )
        .await;
    assert_eq!(
        result,
        ConfirmResult::Confirmed {
            purpose: ChallengePurpose::Registration,
            proof_expires_at: Some(t0() + Duration::minutes(5))
        }
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_correct_confirmations_issue_exactly_one_proof() {
    let fx = Fixture::new().await;
    issue(&fx, "c1", "a", "flow-1").await;
    let mut tasks = Vec::new();
    for index in 0..10 {
        let db = fx.db.clone();
        tasks.push(tokio::spawn(async move {
            db.transaction(move |conn| {
                Box::pin(async move {
                    let proof_hash = format!("proof-{index}");
                    confirm_email_challenge(
                        &PostgresRegistrationEmailChallengeRepository,
                        conn,
                        &ConfirmRequest {
                            id: "c1",
                            expected_purpose: ChallengePurpose::Registration,
                            given_code_hash: "code-c1",
                            new_proof_hash: &proof_hash,
                            current_generation: &ConfigGeneration::new("g1"),
                        },
                        &policy(),
                        t0() + Duration::minutes(1),
                    )
                    .await
                })
            })
            .await
            .unwrap()
        }));
    }
    let mut confirmed = 0;
    for task in tasks {
        if matches!(task.await.unwrap(), ConfirmResult::Confirmed { .. }) {
            confirmed += 1;
        }
    }
    assert_eq!(confirmed, 1, "exactly one concurrent confirmation wins");
    assert_eq!(
        fx.count("registration_email_challenges WHERE proof_hash IS NOT NULL")
            .await,
        1
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn canary_confirmation_issues_no_proof_and_cannot_be_used_for_registration() {
    let fx = Fixture::new().await;
    let new = new_challenge("canary-1", "canary", "canary", "s");
    fx.db
        .transaction(move |conn| {
            Box::pin(async move {
                insert_canary_challenge(
                    &PostgresRegistrationEmailChallengeRepository,
                    conn,
                    new,
                    &policy(),
                    t0(),
                )
                .await
            })
        })
        .await
        .unwrap();
    // registration として確認しようとしても拒否される (canary の code は他の登録者の所有証明にならない)。
    assert_eq!(
        fx.confirm(
            "canary-1",
            ChallengePurpose::Registration,
            "code-canary-1",
            "p",
            "g1",
            t0()
        )
        .await,
        ConfirmResult::Rejected(ConfirmRejection::PurposeMismatch)
    );
    assert_eq!(
        fx.confirm(
            "canary-1",
            ChallengePurpose::Canary,
            "code-canary-1",
            "p",
            "g1",
            t0()
        )
        .await,
        ConfirmResult::Confirmed {
            purpose: ChallengePurpose::Canary,
            proof_expires_at: None
        }
    );
    assert_eq!(
        fx.count("registration_email_challenges WHERE proof_hash IS NOT NULL")
            .await,
        0
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn invalidated_challenges_cannot_be_confirmed() {
    let fx = Fixture::new().await;
    issue(&fx, "c1", "a", "flow-1").await;
    fx.db
        .transaction(|conn| {
            Box::pin(async move {
                invalidate_challenge(&PostgresRegistrationEmailChallengeRepository, conn, "c1")
                    .await
            })
        })
        .await
        .unwrap();
    assert_eq!(
        fx.confirm(
            "c1",
            ChallengePurpose::Registration,
            "code-c1",
            "p",
            "g1",
            t0()
        )
        .await,
        ConfirmResult::Rejected(ConfirmRejection::Challenge(ChallengeRejection::NotPending))
    );
}

// ---- proof の照合・消費 -----------------------------------------------------------------------

async fn confirmed_proof(fx: &Fixture, id: &str, email: &str, flow: &str, proof_hash: &str) {
    issue(fx, id, email, flow).await;
    let result = fx
        .confirm(
            id,
            ChallengePurpose::Registration,
            &format!("code-{id}"),
            proof_hash,
            "g1",
            t0() + Duration::minutes(1),
        )
        .await;
    assert!(matches!(result, ConfirmResult::Confirmed { .. }));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn proof_is_consumed_once_and_only_the_same_binding_can_retry_within_the_window() {
    let fx = Fixture::new().await;
    confirmed_proof(&fx, "c1", "a", "flow-1", "proof-1").await;
    let at = t0() + Duration::minutes(2);
    let b = binding("flow-1", "a", "g1");

    assert_eq!(
        fx.consume("proof-1", b.clone(), at).await,
        ProofVerification::Verified { replayed: false }
    );
    // Kratos の transport retry (再利用窓内、同じ flow/email/generation) は同じ検証済み事実を返す。
    assert_eq!(
        fx.consume("proof-1", b.clone(), at + Duration::seconds(30))
            .await,
        ProofVerification::Verified { replayed: true }
    );
    assert_eq!(
        fx.consume("proof-1", b.clone(), at + Duration::seconds(59))
            .await,
        ProofVerification::Verified { replayed: true }
    );
    // 窓の外は replay として拒否。
    for later in [Duration::seconds(60), Duration::minutes(5)] {
        assert_eq!(
            fx.consume("proof-1", b.clone(), at + later).await,
            ProofVerification::Rejected(ProofRejection::AlreadyConsumed)
        );
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn proof_rejects_other_flow_email_generation_tampering_and_expiry() {
    let fx = Fixture::new().await;
    confirmed_proof(&fx, "c1", "a", "flow-1", "proof-1").await;
    let at = t0() + Duration::minutes(2);

    for (binding, expected) in [
        (binding("flow-2", "a", "g1"), ProofRejection::FlowMismatch),
        (binding("flow-1", "b", "g1"), ProofRejection::EmailMismatch),
        (
            binding("flow-1", "a", "g0"),
            ProofRejection::GenerationMismatch,
        ),
    ] {
        assert_eq!(
            fx.consume("proof-1", binding, at).await,
            ProofVerification::Rejected(expected)
        );
    }
    // 改変・未発行の proof。
    assert_eq!(
        fx.consume("proof-1-tampered", binding("flow-1", "a", "g1"), at)
            .await,
        ProofVerification::Rejected(ProofRejection::Unknown)
    );
    // 拒否は proof を消費しない (その後に正しい束縛で 1 回だけ成功する)。
    assert_eq!(
        fx.consume("proof-1", binding("flow-1", "a", "g1"), at)
            .await,
        ProofVerification::Verified { replayed: false }
    );

    // 期限境界: 直前は有効、ちょうどは期限切れ。
    confirmed_proof(&fx, "c2", "b", "flow-2", "proof-2").await;
    let confirmed_at = t0() + Duration::minutes(1);
    let expires = confirmed_at + policy().proof_ttl;
    assert_eq!(
        fx.consume("proof-2", binding("flow-2", "b", "g1"), expires)
            .await,
        ProofVerification::Rejected(ProofRejection::Expired)
    );
    assert_eq!(
        fx.consume(
            "proof-2",
            binding("flow-2", "b", "g1"),
            expires - Duration::seconds(1)
        )
        .await,
        ProofVerification::Verified { replayed: false }
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_consumption_is_atomic_and_a_replay_on_another_flow_is_rejected() {
    let fx = Fixture::new().await;
    confirmed_proof(&fx, "c1", "a", "flow-1", "proof-1").await;
    let at = t0() + Duration::minutes(2);
    let mut tasks = Vec::new();
    for index in 0..12 {
        let db = fx.db.clone();
        // 4 つに 1 つは別 flow への replay。
        let flow = if index % 4 == 3 {
            "flow-attacker"
        } else {
            "flow-1"
        };
        let binding = binding(flow, "a", "g1");
        tasks.push(tokio::spawn(async move {
            db.transaction(move |conn| {
                Box::pin(async move {
                    consume_registration_proof(
                        &PostgresRegistrationEmailChallengeRepository,
                        conn,
                        "proof-1",
                        &binding,
                        &policy(),
                        at,
                    )
                    .await
                })
            })
            .await
            .unwrap()
        }));
    }
    let (mut first, mut replayed, mut rejected) = (0, 0, 0);
    for task in tasks {
        match task.await.unwrap() {
            ProofVerification::Verified { replayed: false } => first += 1,
            ProofVerification::Verified { replayed: true } => replayed += 1,
            ProofVerification::Rejected(ProofRejection::FlowMismatch) => rejected += 1,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(first, 1, "the proof is consumed exactly once");
    assert_eq!(replayed, 8);
    assert_eq!(rejected, 3);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_proof_never_verifies_a_different_challenge() {
    let fx = Fixture::new().await;
    confirmed_proof(&fx, "c1", "a", "flow-1", "proof-1").await;
    confirmed_proof(&fx, "c2", "b", "flow-2", "proof-2").await;
    let at = t0() + Duration::minutes(2);
    // 他の登録者の proof を自分の flow/email で使えない。
    assert_eq!(
        fx.consume("proof-2", binding("flow-1", "a", "g1"), at)
            .await,
        ProofVerification::Rejected(ProofRejection::FlowMismatch)
    );
    assert_eq!(
        fx.consume("proof-1", binding("flow-2", "b", "g1"), at)
            .await,
        ProofVerification::Rejected(ProofRejection::FlowMismatch)
    );
}
