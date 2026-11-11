//! 実 DB (DATABASE_URL) + ローカル fake receiver による Webhook 配送テスト。
//! `test_with::env` により DATABASE_URL 未設定時は skip されるが、CI の check-driver は
//! PostgreSQL service 付きで必ず実行する (skip のみの成功は不可)。

use super::PostgresWebhookDeliveryRepository;
use crate::database::{PostgresDatabase, REGISTRATION_GATE_DB_TEST_LOCK};
use crate::webhook::{
    AeadWebhookSecretCipher, HostResolver, HttpWebhookSender, WebhookNetworkSettings,
    HEADER_DELIVERY_ATTEMPT, HEADER_EVENT_ID, HEADER_EVENT_TYPE, HEADER_SIGNATURE,
    HEADER_TIMESTAMP,
};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::repository::{
    apply_gate_command, apply_gate_observation, run_webhook_delivery_cycle, send_webhook_test,
    DependOnRegistrationGateRepository, RegistrationGateRepository, WebhookCycleEnv,
    WebhookCycleReport, WebhookDeliveryRepository, WebhookSender, WebhookTestOutcome,
};
use kernel::prelude::entity::{
    AuthAccountId, GateCommand, GateEventKind, ProbeStatus, ProtectionObservation,
    RegistrationGateEventId, RegistrationGateState, RegistrationObservation, WebhookConfigStatus,
    WebhookDeliveryPolicy, WebhookDeliveryStatus, WebhookEndpointId, WebhookFailureReason,
    WebhookResultClass,
};
use sqlx::types::time::OffsetDateTime;
use sqlx::PgConnection;
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use time::Duration as TimeDuration;
use wiremock::matchers::{any, method};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const SECRET: &str = "whsec_integration_secret_0123456789";

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

fn now() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    now.replace_nanosecond(now.nanosecond() / 1000 * 1000)
        .unwrap()
}

fn fast_settings() -> WebhookNetworkSettings {
    WebhookNetworkSettings {
        resolve_timeout: Duration::from_millis(500),
        connect_timeout: Duration::from_millis(500),
        total_timeout: Duration::from_millis(600),
        max_response_bytes: 64 * 1024,
    }
}

struct Fixture {
    db: PostgresDatabase,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    async fn new() -> Self {
        let guard = REGISTRATION_GATE_DB_TEST_LOCK.lock().await;
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.expect("DATABASE_URL set");
        let mut conn = db.connection().await.unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query(
            "TRUNCATE registration_gate_webhook_attempts, registration_gate_webhook_deliveries, \
             registration_gate_webhook_endpoints, registration_gate_webhook_endpoint_audit, \
             registration_gate_webhook_env_import, registration_gate_webhook_state, registration_gate_event_reads, \
             registration_gate_events, registration_gate_audit, registration_gate_state",
        )
        .execute(con)
        .await
        .unwrap();
        Self { db, _guard: guard }
    }

    fn sender(&self, server: &MockServer) -> HttpWebhookSender {
        HttpWebhookSender::from_values_allowing_insecure_loopback(
            &format!("{}/hook", server.uri()),
            SECRET,
        )
        .with_settings(fast_settings())
    }

    async fn gate_state(&self) -> RegistrationGateState {
        let mut conn = self.db.connection().await.unwrap();
        self.db
            .registration_gate_repository()
            .load(&mut conn)
            .await
            .unwrap()
    }

    async fn command(&self, command: GateCommand) {
        let revision = self.gate_state().await.revision;
        let actor = AuthAccountId::default();
        let mut conn = self.db.connection().await.unwrap();
        apply_gate_command(
            self.db.registration_gate_repository(),
            &mut conn,
            revision,
            command,
            &actor,
            now(),
        )
        .await
        .unwrap();
    }

    async fn observe(&self, observation: RegistrationObservation) {
        let mut conn = self.db.connection().await.unwrap();
        apply_gate_observation(
            self.db.registration_gate_repository(),
            &mut conn,
            observation,
            now(),
        )
        .await
        .unwrap();
    }

    /// Provider/override and deployment-attestation event coverage.
    async fn emit_all_kinds(&self) {
        self.command(ACK).await;
        self.command(GateCommand::RevokeOverride).await;
        self.observe(obs(H, OFF)).await;
        self.command(GateCommand::Open).await;
        self.observe(obs(DOWN, OFF)).await;
        self.observe(obs(H, OFF)).await;
        self.observe(obs(H, DOWN)).await;
        self.observe(OFF_OBSERVATION).await;
        let mut observation = OFF_OBSERVATION;
        observation.enforcement_verified = false;
        self.observe(observation).await;
        self.observe(OFF_OBSERVATION).await;
    }

    async fn event_ids_and_kinds(&self) -> Vec<(i64, String)> {
        let mut conn = self.db.connection().await.unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query_as::<_, (i64, String)>(
            "SELECT id, kind FROM registration_gate_events ORDER BY id",
        )
        .fetch_all(con)
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

    async fn cycle(
        &self,
        sender: &HttpWebhookSender,
        worker: &str,
        policy: &WebhookDeliveryPolicy,
        at: OffsetDateTime,
    ) -> WebhookCycleReport {
        let clock = move || at;
        let jitter = || 1.0;
        let stop = || false;
        run_webhook_delivery_cycle(
            &self.db,
            &PostgresWebhookDeliveryRepository,
            sender,
            &AeadWebhookSecretCipher::not_configured(),
            worker,
            policy,
            &WebhookCycleEnv {
                clock: &clock,
                jitter: &jitter,
                should_stop: &stop,
            },
        )
        .await
        .unwrap()
    }

    async fn deliveries(&self) -> Vec<kernel::prelude::entity::WebhookDelivery> {
        let mut conn = self.db.connection().await.unwrap();
        PostgresWebhookDeliveryRepository
            .list_recent(&mut conn, None, 100)
            .await
            .unwrap()
    }

    async fn delivery(&self, event_id: i64) -> kernel::prelude::entity::WebhookDelivery {
        self.deliveries()
            .await
            .into_iter()
            .find(|delivery| *delivery.event_id.as_ref() == event_id)
            .expect("delivery exists")
    }

    async fn attempts(&self, event_id: i64) -> Vec<(u32, WebhookResultClass, Option<u16>)> {
        let mut conn = self.db.connection().await.unwrap();
        PostgresWebhookDeliveryRepository
            .list_attempts(
                &mut conn,
                WebhookEndpointId::ENVIRONMENT,
                &RegistrationGateEventId::new(event_id),
            )
            .await
            .unwrap()
            .into_iter()
            .map(|attempt| (attempt.attempt, attempt.result, attempt.http_status))
            .collect()
    }

    async fn retry_failed(&self, event_id: Option<i64>, at: OffsetDateTime) -> u64 {
        let mut conn = self.db.connection().await.unwrap();
        let id = event_id.map(RegistrationGateEventId::new);
        PostgresWebhookDeliveryRepository
            .retry_failed(&mut conn, None, id.as_ref(), at)
            .await
            .unwrap()
    }
}

const OFF_OBSERVATION: RegistrationObservation = RegistrationObservation::UNCONFIGURED;

struct Received {
    event_id: String,
    event_type: String,
    attempt: u32,
    signature: String,
    timestamp: i64,
    body: Vec<u8>,
}

async fn received(server: &MockServer) -> Vec<Received> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .map(|request| {
            let header = |name: &str| {
                request
                    .headers
                    .get(name)
                    .unwrap_or_else(|| panic!("missing {name}"))
                    .to_str()
                    .unwrap()
                    .to_string()
            };
            Received {
                event_id: header(HEADER_EVENT_ID),
                event_type: header(HEADER_EVENT_TYPE),
                attempt: header(HEADER_DELIVERY_ATTEMPT).parse().unwrap(),
                signature: header(HEADER_SIGNATURE),
                timestamp: header(HEADER_TIMESTAMP).parse().unwrap(),
                body: request.body.clone(),
            }
        })
        .collect()
}

async fn mount_status(server: &MockServer, status: u16) {
    Mock::given(any())
        .respond_with(ResponseTemplate::new(status))
        .mount(server)
        .await;
}

fn default_policy() -> WebhookDeliveryPolicy {
    WebhookDeliveryPolicy::default()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn every_event_kind_is_delivered_once_with_stable_id_signature_and_allowlisted_payload() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 204).await;
    let sender = fx.sender(&server);
    let policy = default_policy();
    let t0 = now();

    // Webhook が有効と観測された時点が baseline になる。それ以降のイベントが配送対象。
    let report = fx.cycle(&sender, "worker-a", &policy, t0).await;
    assert_eq!(report.ingested, 0);
    fx.emit_all_kinds().await;
    let events = fx.event_ids_and_kinds().await;
    let kinds: BTreeSet<_> = events.iter().map(|(_, kind)| kind.as_str()).collect();
    assert_eq!(
        kinds,
        BTreeSet::from([
            "outage_recovered",
            "outage_started",
            "override_revoked",
            "override_started",
            "protection_degraded",
            "protection_lost_stopped",
            "enforcement_lost_stopped",
            "enforcement_restored",
        ]),
        "all provider, override, and enforcement loss/restoration events must have been produced"
    );

    let report = fx.cycle(&sender, "worker-a", &policy, t0).await;
    assert_eq!(report.ingested as usize, events.len());
    assert_eq!(report.delivered as usize, events.len());

    let received = received(&server).await;
    assert_eq!(received.len(), events.len(), "exactly once each");
    let by_id: BTreeMap<_, _> = received.iter().map(|r| (r.event_id.clone(), r)).collect();
    for (id, kind) in &events {
        let delivered = by_id.get(&id.to_string()).expect("event delivered");
        assert_eq!(&delivered.event_type, kind);
        assert_eq!(delivered.attempt, 1);
        assert_eq!(
            crate::webhook::signature::verify(
                SECRET.as_bytes(),
                &delivered.signature,
                delivered.timestamp,
                &delivered.body,
                delivered.timestamp,
                300
            ),
            Ok(())
        );
        let body: serde_json::Value = serde_json::from_slice(&delivered.body).unwrap();
        assert_eq!(body["event_id"], id.to_string());
        assert_eq!(body["type"], kind.as_str());
        if kind.starts_with("enforcement_") {
            assert_eq!(body["gate"]["enforcement_lost_latch"], true);
        }
        let keys: BTreeSet<_> = body.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            BTreeSet::from(
                [
                    "version",
                    "event_id",
                    "type",
                    "occurred_at",
                    "gate",
                    "email_verification",
                    "bot_protection"
                ]
                .map(String::from)
            )
        );
        let text = String::from_utf8(delivered.body.clone()).unwrap();
        for forbidden in ["@", "actor", "secret", SECRET, "auth_account", "password"] {
            assert!(!text.contains(forbidden), "{text}");
        }
        assert_eq!(
            fx.delivery(*id).await.status,
            WebhookDeliveryStatus::Delivered
        );
        assert_eq!(
            fx.attempts(*id).await,
            vec![(1, WebhookResultClass::Delivered, Some(204))]
        );
    }

    let override_started = events
        .iter()
        .find(|(_, kind)| kind == "override_started")
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&by_id[&override_started.0.to_string()].body).unwrap();
    assert_eq!(body["gate"]["override_active"], true);

    // 全件 delivered になった後は何も再送しない
    let report = fx
        .cycle(&sender, "worker-a", &policy, t0 + TimeDuration::days(1))
        .await;
    assert_eq!((report.attempted, report.ingested), (0, 0));
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        events.len()
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn events_before_enablement_are_not_delivered_and_a_disabled_gap_is_caught_up() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 200).await;
    let sender = fx.sender(&server);
    let disabled = HttpWebhookSender::from_values(None, None, None);
    let policy = default_policy();
    let t0 = now();

    fx.command(ACK).await;
    let before_enablement = fx.event_ids_and_kinds().await;
    assert_eq!(before_enablement.len(), 1);

    // 無効 (未設定) の間は何もせず baseline も作らない
    let report = fx.cycle(&disabled, "worker", &policy, t0).await;
    assert!(report.skipped);
    assert_eq!(fx.count("registration_gate_webhook_state").await, 0);

    // 最初に有効になった時点の baseline より前のイベントは配送しない
    let report = fx.cycle(&sender, "worker", &policy, t0).await;
    assert!(!report.skipped);
    assert_eq!(report.ingested, 0);
    assert_eq!(fx.count("registration_gate_webhook_state").await, 1);
    assert!(server.received_requests().await.unwrap().is_empty());

    // 一度有効になった後は、無効だった期間に発生したイベントも取りこぼさず配送する
    fx.command(GateCommand::RevokeOverride).await;
    let during_gap = fx.event_ids_and_kinds().await;
    let during_gap_ids: Vec<_> = during_gap
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| *id != before_enablement[0].0)
        .collect();
    assert!(!during_gap_ids.is_empty());
    let report = fx.cycle(&disabled, "worker", &policy, t0).await;
    assert!(report.skipped);
    assert!(server.received_requests().await.unwrap().is_empty());

    fx.cycle(&sender, "worker", &policy, t0).await;
    let delivered: BTreeSet<_> = received(&server)
        .await
        .into_iter()
        .map(|r| r.event_id)
        .collect();
    let expected: BTreeSet<_> = during_gap_ids.iter().map(|id| id.to_string()).collect();
    assert_eq!(delivered, expected);
    assert!(!delivered.contains(&before_enablement[0].0.to_string()));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn ingestion_is_idempotent_even_when_run_concurrently() {
    let fx = Fixture::new().await;
    let t0 = now();
    {
        let mut conn = fx.db.connection().await.unwrap();
        PostgresWebhookDeliveryRepository
            .ingest_gate_events(&mut conn, t0, 100, true)
            .await
            .unwrap();
    }
    fx.emit_all_kinds().await;
    let events = fx.event_ids_and_kinds().await.len() as i64;

    let ingest = |_: u8| async {
        let mut conn = fx.db.connection().await.unwrap();
        PostgresWebhookDeliveryRepository
            .ingest_gate_events(&mut conn, t0, 100, true)
            .await
    };
    let (a, b, c) = tokio::join!(ingest(1), ingest(2), ingest(3));
    // 同時取り込みで主キー衝突のエラーにならない (ON CONFLICT DO NOTHING)
    let total: u64 = [a, b, c].into_iter().map(|r| r.unwrap().ingested).sum();
    assert_eq!(total as i64, events);
    assert_eq!(
        fx.count("registration_gate_webhook_deliveries").await,
        events
    );
    let again = ingest(4).await.unwrap();
    assert_eq!(again.ingested, 0);
    assert_eq!(
        fx.count("registration_gate_webhook_deliveries").await,
        events
    );
}

/// 一定の Retry-After を付ける fake receiver の応答を順番に返す。
struct Script {
    responses: std::sync::Mutex<std::collections::VecDeque<ResponseTemplate>>,
    fallback: u16,
}

impl Respond for Script {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| ResponseTemplate::new(self.fallback))
    }
}

async fn mount_script(server: &MockServer, responses: Vec<ResponseTemplate>, fallback: u16) {
    Mock::given(any())
        .respond_with(Script {
            responses: std::sync::Mutex::new(responses.into()),
            fallback,
        })
        .mount(server)
        .await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn retryable_failures_back_off_honor_retry_after_and_keep_the_same_event_id_and_body() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_script(
        &server,
        vec![
            ResponseTemplate::new(429).insert_header("Retry-After", "120"),
            ResponseTemplate::new(500),
            ResponseTemplate::new(200).set_delay(Duration::from_secs(3)),
            ResponseTemplate::new(408),
        ],
        204,
    )
    .await;
    let sender = fx.sender(&server);
    let policy = default_policy();
    let t0 = now();

    fx.cycle(&sender, "w", &policy, t0).await;
    fx.command(ACK).await;
    let event_id = fx.event_ids_and_kinds().await[0].0;

    // 1 回目: 429 + Retry-After: 120 (バックオフ 30s より長いので 120s を尊重)
    let report = fx.cycle(&sender, "w", &policy, t0).await;
    assert_eq!((report.attempted, report.retrying), (1, 1));
    let delivery = fx.delivery(event_id).await;
    assert_eq!(delivery.status, WebhookDeliveryStatus::Retrying);
    assert_eq!(
        delivery.last_result,
        Some(WebhookResultClass::HttpTooManyRequests)
    );
    assert_eq!(
        delivery.next_attempt_at,
        Some(t0 + TimeDuration::seconds(120))
    );

    // 期限前は送らない
    let report = fx
        .cycle(&sender, "w", &policy, t0 + TimeDuration::seconds(119))
        .await;
    assert_eq!(report.attempted, 0);

    // 2 回目: 500 → バックオフ 60s
    let t1 = t0 + TimeDuration::seconds(120);
    fx.cycle(&sender, "w", &policy, t1).await;
    let delivery = fx.delivery(event_id).await;
    assert_eq!(
        delivery.last_result,
        Some(WebhookResultClass::HttpServerError)
    );
    assert_eq!(
        delivery.next_attempt_at,
        Some(t1 + TimeDuration::seconds(60))
    );

    // 3 回目: timeout (応答が全体 timeout より遅い)
    let t2 = t1 + TimeDuration::seconds(60);
    fx.cycle(&sender, "w", &policy, t2).await;
    let delivery = fx.delivery(event_id).await;
    assert_eq!(delivery.last_result, Some(WebhookResultClass::Timeout));
    assert_eq!(delivery.status, WebhookDeliveryStatus::Retrying);

    // 4 回目: 408 → 再試行、5 回目: 成功
    let t3 = t2 + TimeDuration::minutes(10);
    fx.cycle(&sender, "w", &policy, t3).await;
    assert_eq!(
        fx.delivery(event_id).await.last_result,
        Some(WebhookResultClass::HttpRequestTimeout)
    );
    let t4 = t3 + TimeDuration::minutes(10);
    let report = fx.cycle(&sender, "w", &policy, t4).await;
    assert_eq!(report.delivered, 1);
    let delivery = fx.delivery(event_id).await;
    assert_eq!(delivery.status, WebhookDeliveryStatus::Delivered);
    assert_eq!(delivery.attempt_count, 5);
    assert_eq!(
        fx.attempts(event_id).await,
        vec![
            (1, WebhookResultClass::HttpTooManyRequests, Some(429)),
            (2, WebhookResultClass::HttpServerError, Some(500)),
            (3, WebhookResultClass::Timeout, None),
            (4, WebhookResultClass::HttpRequestTimeout, Some(408)),
            (5, WebhookResultClass::Delivered, Some(204)),
        ]
    );

    // 全試行で event ID・本文は同一、試行番号だけが増える
    let received = received(&server).await;
    assert_eq!(received.len(), 5);
    assert!(received.iter().all(|r| r.event_id == event_id.to_string()));
    assert!(received.iter().all(|r| r.body == received[0].body));
    assert_eq!(
        received.iter().map(|r| r.attempt).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn retries_are_bounded_and_exhaustion_fails_until_the_admin_retries() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 503).await;
    let sender = fx.sender(&server);
    let policy = WebhookDeliveryPolicy {
        max_attempts: 3,
        ..default_policy()
    };
    let t0 = now();
    fx.cycle(&sender, "w", &policy, t0).await;
    fx.command(ACK).await;
    let event_id = fx.event_ids_and_kinds().await[0].0;

    let mut at = t0;
    for _ in 0..6 {
        fx.cycle(&sender, "w", &policy, at).await;
        at += TimeDuration::hours(2);
    }
    let delivery = fx.delivery(event_id).await;
    assert_eq!(delivery.status, WebhookDeliveryStatus::Failed);
    assert_eq!(
        delivery.failure_reason,
        Some(WebhookFailureReason::RetriesExhausted)
    );
    assert_eq!(
        delivery.last_result,
        Some(WebhookResultClass::HttpServerError)
    );
    assert_eq!(delivery.attempt_count, 3);
    assert_eq!(server.received_requests().await.unwrap().len(), 3);

    // 管理者が再試行するまで再送されない
    fx.cycle(&sender, "w", &policy, at + TimeDuration::days(7))
        .await;
    assert_eq!(server.received_requests().await.unwrap().len(), 3);

    // 再試行すると同じ event ID をもう一度サイクル分 (最大 3 回) 送る。通算試行番号は増え続ける。
    server.reset().await;
    mount_status(&server, 204).await;
    assert_eq!(fx.retry_failed(None, at).await, 1);
    let delivery = fx.delivery(event_id).await;
    assert_eq!(delivery.status, WebhookDeliveryStatus::Pending);
    assert_eq!(delivery.failure_reason, None);
    fx.cycle(&sender, "w", &policy, at).await;
    let delivery = fx.delivery(event_id).await;
    assert_eq!(delivery.status, WebhookDeliveryStatus::Delivered);
    assert_eq!(delivery.attempt_count, 4);
    let received = received(&server).await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].event_id, event_id.to_string());
    assert_eq!(received[0].attempt, 4);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn permanent_errors_fail_immediately_and_only_an_admin_retry_resends() {
    for (status, class) in [
        (404u16, WebhookResultClass::HttpClientError),
        (400, WebhookResultClass::HttpClientError),
        (401, WebhookResultClass::HttpClientError),
        (302, WebhookResultClass::HttpRedirect),
        (301, WebhookResultClass::HttpRedirect),
    ] {
        let fx = Fixture::new().await;
        let server = MockServer::start().await;
        let internal = MockServer::start().await;
        mount_status(&internal, 200).await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(status).insert_header("Location", internal.uri().as_str()),
            )
            .mount(&server)
            .await;
        let sender = fx.sender(&server);
        let policy = default_policy();
        let t0 = now();
        fx.cycle(&sender, "w", &policy, t0).await;
        fx.command(ACK).await;
        let event_id = fx.event_ids_and_kinds().await[0].0;

        let report = fx.cycle(&sender, "w", &policy, t0).await;
        assert_eq!((report.attempted, report.failed), (1, 1), "status {status}");
        let delivery = fx.delivery(event_id).await;
        assert_eq!(delivery.status, WebhookDeliveryStatus::Failed);
        assert_eq!(
            delivery.failure_reason,
            Some(WebhookFailureReason::PermanentError)
        );
        assert_eq!(delivery.last_result, Some(class));
        assert_eq!(delivery.last_http_status, Some(status));

        fx.cycle(&sender, "w", &policy, t0 + TimeDuration::days(30))
            .await;
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "no automatic retry for {status}"
        );
        assert!(
            internal.received_requests().await.unwrap().is_empty(),
            "redirect target must never be contacted"
        );

        // 受信側を直して管理者が再試行 → 同じ event ID が再送される
        server.reset().await;
        mount_status(&server, 204).await;
        assert_eq!(fx.retry_failed(Some(event_id), t0).await, 1);
        fx.cycle(&sender, "w", &policy, t0).await;
        assert_eq!(
            fx.delivery(event_id).await.status,
            WebhookDeliveryStatus::Delivered
        );
        let received = received(&server).await;
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].event_id, event_id.to_string());
        assert_eq!(fx.attempts(event_id).await.len(), 2);
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn retry_failed_only_touches_failed_deliveries() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 204).await;
    let sender = fx.sender(&server);
    let policy = default_policy();
    let t0 = now();
    fx.cycle(&sender, "w", &policy, t0).await;
    fx.command(ACK).await;
    fx.cycle(&sender, "w", &policy, t0).await;
    let event_id = fx.event_ids_and_kinds().await[0].0;
    assert_eq!(
        fx.delivery(event_id).await.status,
        WebhookDeliveryStatus::Delivered
    );
    assert_eq!(fx.retry_failed(Some(event_id), t0).await, 0);
    assert_eq!(fx.retry_failed(None, t0).await, 0);
    assert_eq!(fx.retry_failed(Some(123), t0).await, 0);
    fx.cycle(&sender, "w", &policy, t0 + TimeDuration::hours(1))
        .await;
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

struct FixedResolver(Vec<SocketAddr>);

impl HostResolver for FixedResolver {
    fn resolve<'a>(
        &'a self,
        _host: &'a str,
        _port: u16,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + 'a>,
    > {
        let addresses = self.0.clone();
        Box::pin(async move { Ok(addresses) })
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn ssrf_rejection_is_recorded_as_a_permanent_failure_without_any_request() {
    let fx = Fixture::new().await;
    let receiver = MockServer::start().await;
    mount_status(&receiver, 200).await;
    // production 設定 (厳格) のまま、名前が内部アドレスへ解決される (DNS rebinding 相当)
    let sender =
        HttpWebhookSender::from_values(Some("https://hooks.example.com/hook"), Some(SECRET), None)
            .with_settings(fast_settings())
            .with_resolver(Arc::new(FixedResolver(vec![SocketAddr::new(
                "127.0.0.1".parse().unwrap(),
                receiver.address().port(),
            )])));
    let policy = default_policy();
    let t0 = now();
    fx.cycle(&sender, "w", &policy, t0).await;
    fx.command(ACK).await;
    let event_id = fx.event_ids_and_kinds().await[0].0;

    fx.cycle(&sender, "w", &policy, t0).await;
    let delivery = fx.delivery(event_id).await;
    assert_eq!(delivery.status, WebhookDeliveryStatus::Failed);
    assert_eq!(
        delivery.failure_reason,
        Some(WebhookFailureReason::PermanentError)
    );
    assert_eq!(delivery.last_result, Some(WebhookResultClass::SsrfBlocked));
    assert_eq!(delivery.last_http_status, None);
    assert!(receiver.received_requests().await.unwrap().is_empty());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_crashed_worker_is_recovered_after_lease_expiry_without_losing_the_event() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 204).await;
    let sender = fx.sender(&server);
    let policy = default_policy();
    let t0 = now();
    fx.cycle(&sender, "worker-old", &policy, t0).await;
    fx.command(ACK).await;
    let event_id = fx.event_ids_and_kinds().await[0].0;

    // worker-old が lease を取った後、送信結果を記録せずに停止した (再起動を模擬)
    let claim = {
        let mut conn = fx.db.connection().await.unwrap();
        let repository = PostgresWebhookDeliveryRepository;
        repository
            .ingest_gate_events(&mut conn, t0, 100, true)
            .await
            .unwrap();
        let claims = repository
            .claim_due(
                &mut conn,
                WebhookEndpointId::ENVIRONMENT,
                "worker-old",
                t0,
                &policy,
            )
            .await
            .unwrap();
        assert_eq!(claims.len(), 1);
        claims.into_iter().next().unwrap()
    };
    let delivery = fx.delivery(event_id).await;
    assert_eq!(delivery.status, WebhookDeliveryStatus::InFlight);
    assert_eq!(delivery.lease_expires_at, Some(t0 + policy.lease_duration));

    // lease 期限内は他 worker が取らない
    let report = fx
        .cycle(
            &sender,
            "worker-new",
            &policy,
            t0 + policy.lease_duration - TimeDuration::seconds(1),
        )
        .await;
    assert_eq!(report.attempted, 0);
    assert!(server.received_requests().await.unwrap().is_empty());

    // 期限切れ → 新しい worker (再起動後のプロセス) が回収して配送する
    let t1 = t0 + policy.lease_duration + TimeDuration::seconds(1);
    let report = fx.cycle(&sender, "worker-new", &policy, t1).await;
    assert_eq!((report.attempted, report.delivered), (1, 1));
    let delivery = fx.delivery(event_id).await;
    assert_eq!(delivery.status, WebhookDeliveryStatus::Delivered);
    assert_eq!(delivery.attempt_count, 2);
    assert_eq!(
        fx.attempts(event_id).await,
        vec![
            (1, WebhookResultClass::LeaseExpired, None),
            (2, WebhookResultClass::Delivered, Some(204)),
        ]
    );
    let received = received(&server).await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].event_id, event_id.to_string());

    // 遅れて戻ってきた古い worker の結果は lease を失っているので記録されない (二重記録なし)
    let stale = {
        let mut conn = fx.db.connection().await.unwrap();
        PostgresWebhookDeliveryRepository
            .record_attempt(
                &mut conn,
                &claim,
                "worker-old",
                t1,
                &kernel::prelude::entity::WebhookDecision::Failed {
                    class: WebhookResultClass::HttpServerError,
                    http_status: Some(500),
                    reason: WebhookFailureReason::PermanentError,
                },
            )
            .await
            .unwrap()
    };
    assert!(!stale);
    assert_eq!(
        fx.delivery(event_id).await.status,
        WebhookDeliveryStatus::Delivered
    );
    assert_eq!(fx.attempts(event_id).await.len(), 2);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_recovered_delivery_over_the_attempt_budget_fails_instead_of_resending() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 204).await;
    let sender = fx.sender(&server);
    let policy = WebhookDeliveryPolicy {
        max_attempts: 1,
        ..default_policy()
    };
    let t0 = now();
    fx.cycle(&sender, "w", &policy, t0).await;
    fx.command(ACK).await;
    let event_id = fx.event_ids_and_kinds().await[0].0;
    {
        let mut conn = fx.db.connection().await.unwrap();
        let repository = PostgresWebhookDeliveryRepository;
        repository
            .ingest_gate_events(&mut conn, t0, 100, true)
            .await
            .unwrap();
        repository
            .claim_due(
                &mut conn,
                WebhookEndpointId::ENVIRONMENT,
                "crashed",
                t0,
                &policy,
            )
            .await
            .unwrap();
    }
    let t1 = t0 + policy.lease_duration + TimeDuration::seconds(1);
    let report = fx.cycle(&sender, "w", &policy, t1).await;
    assert_eq!(report.attempted, 0);
    let delivery = fx.delivery(event_id).await;
    assert_eq!(delivery.status, WebhookDeliveryStatus::Failed);
    assert_eq!(
        delivery.failure_reason,
        Some(WebhookFailureReason::RetriesExhausted)
    );
    assert_eq!(delivery.last_result, Some(WebhookResultClass::LeaseExpired));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_workers_never_send_the_same_event_twice() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_millis(150)))
        .mount(&server)
        .await;
    let sender = fx.sender(&server);
    let policy = WebhookDeliveryPolicy {
        batch_size: 2,
        ..default_policy()
    };
    let t0 = now();
    fx.cycle(&sender, "w", &policy, t0).await;
    fx.emit_all_kinds().await;
    let events = fx.event_ids_and_kinds().await;
    assert!(events.len() >= 6);

    for _round in 0..events.len() {
        let (a, b, c) = tokio::join!(
            fx.cycle(&sender, "worker-a", &policy, t0),
            fx.cycle(&sender, "worker-b", &policy, t0),
            fx.cycle(&sender, "worker-c", &policy, t0),
        );
        assert_eq!(a.lease_lost + b.lease_lost + c.lease_lost, 0);
        if fx
            .deliveries()
            .await
            .iter()
            .all(|d| d.status == WebhookDeliveryStatus::Delivered)
            && fx.deliveries().await.len() == events.len()
        {
            break;
        }
    }

    let received = received(&server).await;
    let mut hits: BTreeMap<String, usize> = BTreeMap::new();
    for request in &received {
        *hits.entry(request.event_id.clone()).or_default() += 1;
    }
    assert_eq!(hits.len(), events.len());
    assert!(
        hits.values().all(|count| *count == 1),
        "each event must be sent exactly once: {hits:?}"
    );
    for (id, _) in &events {
        assert_eq!(
            fx.delivery(*id).await.status,
            WebhookDeliveryStatus::Delivered
        );
        assert_eq!(fx.attempts(*id).await.len(), 1);
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn webhook_trouble_never_changes_the_gate_or_creates_notification_events() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 500).await;
    let failing = fx.sender(&server);
    let misconfigured =
        HttpWebhookSender::from_values(Some("https://10.0.0.1/hook"), Some(SECRET), None);
    let disabled = HttpWebhookSender::from_values(None, None, None);
    let permanent = {
        let permanent_server = MockServer::start().await;
        mount_status(&permanent_server, 410).await;
        (fx.sender(&permanent_server), permanent_server)
    };
    let policy = WebhookDeliveryPolicy {
        max_attempts: 2,
        ..default_policy()
    };
    let t0 = now();
    fx.cycle(&failing, "w", &policy, t0).await;
    fx.emit_all_kinds().await;

    let state_before = fx.gate_state().await;
    let events_before = fx.event_ids_and_kinds().await;
    let audit_before = fx.count("registration_gate_audit").await;
    let reads_before = fx.count("registration_gate_event_reads").await;

    let mut at = t0;
    for _ in 0..5 {
        fx.cycle(&failing, "w", &policy, at).await;
        fx.cycle(&misconfigured, "w", &policy, at).await;
        fx.cycle(&disabled, "w", &policy, at).await;
        fx.cycle(&permanent.0, "w", &policy, at).await;
        at += TimeDuration::hours(3);
    }
    let failed = fx
        .deliveries()
        .await
        .into_iter()
        .filter(|d| d.status == WebhookDeliveryStatus::Failed)
        .count();
    assert!(
        failed > 0,
        "scenario must actually produce failed deliveries"
    );

    assert_eq!(fx.gate_state().await, state_before);
    assert_eq!(fx.event_ids_and_kinds().await, events_before);
    assert_eq!(fx.count("registration_gate_audit").await, audit_before);
    assert_eq!(
        fx.count("registration_gate_event_reads").await,
        reads_before
    );
    // 配送記録は通知イベントと 1:1。失敗が新しい配送対象を生んでいない
    assert_eq!(
        fx.count("registration_gate_webhook_deliveries").await,
        events_before.len() as i64
    );
    let delivery_ids: BTreeSet<_> = fx
        .deliveries()
        .await
        .iter()
        .map(|d| *d.event_id.as_ref())
        .collect();
    let event_ids: BTreeSet<_> = events_before.iter().map(|(id, _)| *id).collect();
    assert_eq!(delivery_ids, event_ids);

    // 配送対象は DB 制約でも通知イベント種別に限られる (テスト送信や失敗通知の種別は入れられない)
    let mut conn = fx.db.connection().await.unwrap();
    let con: &mut PgConnection = &mut conn;
    for kind in ["webhook_test", "delivery_failed"] {
        let inserted = sqlx::query(
            "INSERT INTO registration_gate_webhook_deliveries \
             (event_id, kind, payload, status, cycle_started_at, next_attempt_at, created_at, updated_at) \
             VALUES (999, $1, '{}', 'pending', now(), now(), now(), now())",
        )
        .bind(kind)
        .execute(&mut *con)
        .await;
        assert!(inserted.is_err(), "{kind} must not be deliverable");
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn test_send_uses_the_real_path_but_touches_no_gate_state_notification_or_delivery_record() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 204).await;
    let sender = fx.sender(&server);
    let policy = default_policy();
    let t0 = now();
    fx.cycle(&sender, "w", &policy, t0).await;
    fx.emit_all_kinds().await;
    let state_before = fx.gate_state().await;
    let events_before = fx.event_ids_and_kinds().await;
    let audit_before = fx.count("registration_gate_audit").await;

    let destination = sender.environment_destination();
    let outcome = send_webhook_test(&sender, destination.as_ref(), now())
        .await
        .unwrap();
    let WebhookTestOutcome::Sent {
        event_id,
        delivered,
        result,
        http_status,
        ..
    } = outcome
    else {
        panic!("expected a sent test");
    };
    assert!(delivered);
    assert_eq!(result, WebhookResultClass::Delivered);
    assert_eq!(http_status, Some(204));
    assert!(event_id.starts_with("test_"));

    let received = received(&server).await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].event_id, event_id);
    assert_eq!(received[0].event_type, "webhook_test");
    assert_eq!(
        crate::webhook::signature::verify(
            SECRET.as_bytes(),
            &received[0].signature,
            received[0].timestamp,
            &received[0].body,
            received[0].timestamp,
            300
        ),
        Ok(())
    );

    assert_eq!(fx.gate_state().await, state_before);
    assert_eq!(fx.event_ids_and_kinds().await, events_before);
    assert_eq!(fx.count("registration_gate_audit").await, audit_before);
    assert_eq!(fx.count("registration_gate_webhook_deliveries").await, 0);
    assert_eq!(fx.count("registration_gate_webhook_attempts").await, 0);

    // 不達でも結果が返るだけ (配送記録は作らない)
    server.reset().await;
    mount_status(&server, 503).await;
    let WebhookTestOutcome::Sent {
        delivered, result, ..
    } = send_webhook_test(&sender, destination.as_ref(), now())
        .await
        .unwrap()
    else {
        panic!("expected a sent test");
    };
    assert!(!delivered);
    assert_eq!(result, WebhookResultClass::HttpServerError);
    assert_eq!(fx.count("registration_gate_webhook_deliveries").await, 0);

    // 未設定・設定不備の環境変数の送信先は送信先として現れない (use case が NotConfigured /
    // Misconfigured を返す。application のテストで固定)。
    assert_eq!(
        HttpWebhookSender::from_values(None, None, None).status(),
        WebhookConfigStatus::Disabled
    );
    assert!(matches!(
        HttpWebhookSender::from_values(Some("http://x.example/"), Some(SECRET), None).status(),
        WebhookConfigStatus::Misconfigured { .. }
    ));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn summary_reports_counts_and_last_success_and_failure_times() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_script(
        &server,
        vec![ResponseTemplate::new(204), ResponseTemplate::new(503)],
        404,
    )
    .await;
    let sender = fx.sender(&server);
    let policy = default_policy();
    let t0 = now();
    fx.cycle(&sender, "w", &policy, t0).await;
    fx.command(ACK).await;
    fx.command(GateCommand::RevokeOverride).await;
    fx.observe(obs(H, OFF)).await;
    fx.command(GateCommand::Open).await;
    fx.observe(obs(DOWN, OFF)).await;
    let total = fx.event_ids_and_kinds().await.len() as i64;
    assert!(total >= 4);

    fx.cycle(&sender, "w", &policy, t0 + TimeDuration::seconds(5))
        .await;
    let mut conn = fx.db.connection().await.unwrap();
    let stats = PostgresWebhookDeliveryRepository
        .endpoint_stats(&mut conn)
        .await
        .unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].0, WebhookEndpointId::ENVIRONMENT);
    let summary = &stats[0].1;
    let counts = &summary.counts;
    assert_eq!(counts.delivered, 1);
    assert_eq!(counts.retrying, 1);
    assert_eq!(counts.failed, total - 2);
    assert_eq!(counts.pending + counts.in_flight, 0);
    assert_eq!(summary.last_success_at, Some(t0 + TimeDuration::seconds(5)));
    assert_eq!(summary.last_failure_at, Some(t0 + TimeDuration::seconds(5)));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn nothing_secret_or_personal_is_persisted() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 500).await;
    let sender = fx.sender(&server);
    let policy = default_policy();
    let t0 = now();
    fx.cycle(&sender, "w", &policy, t0).await;
    fx.emit_all_kinds().await;
    fx.cycle(&sender, "w", &policy, t0).await;

    let mut conn = fx.db.connection().await.unwrap();
    let con: &mut PgConnection = &mut conn;
    let dump: Vec<(String,)> = sqlx::query_as(
        "SELECT row_to_json(d)::text FROM registration_gate_webhook_deliveries d \
         UNION ALL SELECT row_to_json(a)::text FROM registration_gate_webhook_attempts a \
         UNION ALL SELECT row_to_json(s)::text FROM registration_gate_webhook_state s",
    )
    .fetch_all(con)
    .await
    .unwrap();
    assert!(!dump.is_empty());
    let text = dump
        .into_iter()
        .map(|(row,)| row)
        .collect::<Vec<_>>()
        .join("\n");
    let host = server.address().ip().to_string();
    for forbidden in [
        SECRET,
        "whsec_",
        host.as_str(),
        "http://",
        "https://",
        "@",
        "/hook",
    ] {
        assert!(
            !text.contains(forbidden),
            "persisted data leaks {forbidden}"
        );
    }
    let _ = GateEventKind::OutageStarted;
}

fn sender_at(server: &MockServer, path: &str) -> HttpWebhookSender {
    HttpWebhookSender::from_values_allowing_insecure_loopback(
        &format!("{}{path}", server.uri()),
        SECRET,
    )
    .with_settings(WebhookNetworkSettings {
        total_timeout: Duration::from_secs(5),
        ..fast_settings()
    })
}

/// 送信に (仮想的に) 40 秒かかる fake receiver。`advance` のときだけ共有の仮想時計を進める。
struct SlowSend {
    advance: bool,
    offset_seconds: Arc<std::sync::atomic::AtomicI64>,
    hits: Arc<std::sync::Mutex<BTreeMap<String, usize>>>,
}

impl Respond for SlowSend {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let event_id = request
            .headers
            .get(HEADER_EVENT_ID)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        *self.hits.lock().unwrap().entry(event_id).or_default() += 1;
        if self.advance {
            self.offset_seconds
                .fetch_add(40, std::sync::atomic::Ordering::SeqCst);
        }
        ResponseTemplate::new(204).set_delay(Duration::from_millis(50))
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_batch_that_outlasts_the_lease_is_never_sent_twice_by_another_worker() {
    use std::sync::atomic::{AtomicI64, Ordering};
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    let offset_seconds = Arc::new(AtomicI64::new(0));
    let hits = Arc::new(std::sync::Mutex::new(BTreeMap::new()));
    for (path, advance) in [("/hook-a", true), ("/hook-b", false)] {
        Mock::given(wiremock::matchers::path(path))
            .respond_with(SlowSend {
                advance,
                offset_seconds: offset_seconds.clone(),
                hits: hits.clone(),
            })
            .mount(&server)
            .await;
    }
    let sender_a = sender_at(&server, "/hook-a");
    let sender_b = sender_at(&server, "/hook-b");
    // 10 件のバッチに対して lease は 60 秒。worker A の送信 1 件ごとに仮想時刻が 40 秒進むので、
    // バッチ全体を最初に claim する実装では後ろの配送の lease が A の送信中に切れる。
    let policy = WebhookDeliveryPolicy {
        lease_duration: TimeDuration::seconds(60),
        batch_size: 10,
        ..default_policy()
    };
    let t0 = now();
    let clock_offset = offset_seconds.clone();
    let clock = move || t0 + TimeDuration::seconds(clock_offset.load(Ordering::SeqCst));
    run_with(&fx, &sender_a, "worker-a", &policy, &clock).await;
    fx.emit_all_kinds().await;
    let events = fx.event_ids_and_kinds().await;
    assert!(events.len() >= 6);

    let worker_a = run_with(&fx, &sender_a, "worker-a", &policy, &clock);
    let worker_b = async {
        for _ in 0..12 {
            run_with(&fx, &sender_b, "worker-b", &policy, &clock).await;
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    };
    let (report_a, ()) = tokio::join!(worker_a, worker_b);
    assert_eq!(report_a.lease_lost, 0);
    run_with(&fx, &sender_a, "worker-a", &policy, &clock).await;

    let hits = hits.lock().unwrap().clone();
    assert_eq!(hits.len(), events.len());
    assert!(
        hits.values().all(|count| *count == 1),
        "each event must be sent exactly once: {hits:?}"
    );
    for (id, _) in &events {
        assert_eq!(
            fx.delivery(*id).await.status,
            WebhookDeliveryStatus::Delivered
        );
    }
}

async fn run_with(
    fx: &Fixture,
    sender: &HttpWebhookSender,
    worker: &str,
    policy: &WebhookDeliveryPolicy,
    clock: &(dyn Fn() -> OffsetDateTime + Sync),
) -> WebhookCycleReport {
    let jitter = || 1.0;
    let stop = || false;
    run_webhook_delivery_cycle(
        &fx.db,
        &PostgresWebhookDeliveryRepository,
        sender,
        &AeadWebhookSecretCipher::not_configured(),
        worker,
        policy,
        &WebhookCycleEnv {
            clock,
            jitter: &jitter,
            should_stop: &stop,
        },
    )
    .await
    .unwrap()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn retrying_deliveries_past_the_max_age_are_failed_not_resent_after_a_long_stop() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 503).await;
    let sender = fx.sender(&server);
    let policy = default_policy();
    let t0 = now();
    fx.cycle(&sender, "w", &policy, t0).await;
    fx.command(ACK).await;
    let event_id = fx.event_ids_and_kinds().await[0].0;

    fx.cycle(&sender, "w", &policy, t0).await;
    let delivery = fx.delivery(event_id).await;
    assert_eq!(delivery.status, WebhookDeliveryStatus::Retrying);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);

    // worker が 25 時間止まり、最大期間 (24 時間) を超えてから再開する。受信側が復旧していても再送しない。
    server.reset().await;
    mount_status(&server, 204).await;
    let resumed = t0 + policy.max_age + TimeDuration::hours(1);
    let report = fx.cycle(&sender, "w", &policy, resumed).await;
    assert_eq!(report.attempted, 0);
    let delivery = fx.delivery(event_id).await;
    assert_eq!(delivery.status, WebhookDeliveryStatus::Failed);
    assert_eq!(
        delivery.failure_reason,
        Some(WebhookFailureReason::RetriesExhausted)
    );
    assert_eq!(
        delivery.last_result,
        Some(WebhookResultClass::HttpServerError)
    );
    fx.cycle(&sender, "w", &policy, resumed + TimeDuration::hours(5))
        .await;
    assert!(server.received_requests().await.unwrap().is_empty());

    // 管理者の再試行なら (新しいサイクルとして) 送られる
    assert_eq!(fx.retry_failed(Some(event_id), resumed).await, 1);
    fx.cycle(&sender, "w", &policy, resumed).await;
    assert_eq!(
        fx.delivery(event_id).await.status,
        WebhookDeliveryStatus::Delivered
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_never_attempted_delivery_is_still_sent_after_a_long_outage() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 204).await;
    let sender = fx.sender(&server);
    let policy = default_policy();
    let t0 = now();
    fx.cycle(&sender, "w", &policy, t0).await;
    fx.command(ACK).await;
    let event_id = fx.event_ids_and_kinds().await[0].0;

    // t0 で配送記録だけ取り込み、送信しないまま worker が停止した状態にする
    {
        let mut conn = fx.db.connection().await.unwrap();
        let ingested = PostgresWebhookDeliveryRepository
            .ingest_gate_events(&mut conn, t0, 100, true)
            .await
            .unwrap();
        assert_eq!(ingested.ingested, 1);
    }
    let delivery = fx.delivery(event_id).await;
    assert_eq!(delivery.status, WebhookDeliveryStatus::Pending);
    assert_eq!(delivery.attempt_count, 0);
    let cycle_started_at: OffsetDateTime = {
        let mut conn = fx.db.connection().await.unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query_scalar(
            "SELECT cycle_started_at FROM registration_gate_webhook_deliveries WHERE event_id = $1",
        )
        .bind(event_id)
        .fetch_one(con)
        .await
        .unwrap()
    };
    assert_eq!(cycle_started_at, t0);
    assert!(server.received_requests().await.unwrap().is_empty());

    // 最大期間を超えてから再開しても、未試行 pending の初回送信は行われる
    let resumed = t0 + policy.max_age + TimeDuration::days(2);
    let report = fx.cycle(&sender, "w", &policy, resumed).await;
    assert_eq!((report.attempted, report.delivered), (1, 1));
    assert_eq!(
        fx.delivery(event_id).await.status,
        WebhookDeliveryStatus::Delivered
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

mod endpoint_tests;
