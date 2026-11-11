//! 送信先管理 (E15x) の実 DB テスト。`DATABASE_URL` が必要 (CI の check-driver は PostgreSQL service
//! 付きで必ず実行する。skip のみの成功は不可)。送信先は loopback の fake receiver (wiremock) に向ける
//! (`cfg(test)` 限定の例外。本番ビルドの設定・API からは有効化できない)。

use super::*;
use crate::webhook::signature;
use kernel::interfaces::repository::WebhookSecretCipher;
use kernel::prelude::entity::{
    category_of, NewWebhookEndpoint, StoredWebhookEndpoint, ValidatedWebhookUrl, WebhookAuditEntry,
    WebhookAuditOperation, WebhookAuditOutcome, WebhookCategories, WebhookCategory,
    WebhookEndpointPatch, WebhookEndpointSource, WebhookEndpointWrite, WebhookFailureCursor,
    WebhookSecret, MAX_WEBHOOK_ENDPOINTS,
};
use sqlx::Executor;

const SECRET_A: &str = "endpoint-a-secret-0123456789abcdef";
const SECRET_B: &str = "endpoint-b-secret-fedcba9876543210";
const KEY_1: [u8; 32] = [1u8; 32];
const KEY_2: [u8; 32] = [2u8; 32];

fn cipher_1() -> AeadWebhookSecretCipher {
    AeadWebhookSecretCipher::from_key_bytes(KEY_1)
}

fn cipher_2() -> AeadWebhookSecretCipher {
    AeadWebhookSecretCipher::from_key_bytes(KEY_2)
}

fn endpoint_sender() -> HttpWebhookSender {
    HttpWebhookSender::disabled_allowing_insecure_loopback_destinations()
        .with_settings(fast_settings())
}

fn admin() -> AuthAccountId {
    AuthAccountId::default()
}

async fn add_endpoint(
    fx: &Fixture,
    cipher: &AeadWebhookSecretCipher,
    url: &str,
    secret: &str,
    categories: &[WebhookCategory],
) -> StoredWebhookEndpoint {
    let id = WebhookEndpointId::generate();
    let sealed = cipher
        .seal(id, &WebhookSecret::parse(secret).unwrap())
        .unwrap();
    let new = NewWebhookEndpoint {
        id,
        url: ValidatedWebhookUrl::new_checked(url.to_string()),
        sealed_secret: sealed,
        categories: WebhookCategories::new(categories.iter().copied()).unwrap(),
        source: WebhookEndpointSource::Api,
    };
    let mut conn = fx.db.connection().await.unwrap();
    match PostgresWebhookDeliveryRepository
        .create_endpoint(&mut conn, new, &admin(), now())
        .await
        .unwrap()
    {
        WebhookEndpointWrite::Done(stored) => *stored,
        other => panic!("create_endpoint: {other:?}"),
    }
}

async fn run_cycle(
    fx: &Fixture,
    sender: &HttpWebhookSender,
    cipher: &AeadWebhookSecretCipher,
    policy: &WebhookDeliveryPolicy,
    at: OffsetDateTime,
) -> WebhookCycleReport {
    let clock = move || at;
    let jitter = || 1.0;
    let stop = || false;
    run_webhook_delivery_cycle(
        &fx.db,
        &PostgresWebhookDeliveryRepository,
        sender,
        cipher,
        "worker-endpoints",
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

async fn deliveries_of(
    fx: &Fixture,
    endpoint: WebhookEndpointId,
) -> Vec<kernel::prelude::entity::WebhookDelivery> {
    let mut conn = fx.db.connection().await.unwrap();
    PostgresWebhookDeliveryRepository
        .list_recent(&mut conn, Some(endpoint), 1000)
        .await
        .unwrap()
}

async fn mount_ok(server: &MockServer) {
    mount_status(server, 204).await;
}

fn event_id_set(received: &[Received]) -> BTreeSet<String> {
    received.iter().map(|r| r.event_id.clone()).collect()
}

async fn insert_failed(fx: &Fixture, endpoint_id: i64, event_id: i64, failed_at: OffsetDateTime) {
    let mut conn = fx.db.connection().await.unwrap();
    let con: &mut PgConnection = &mut conn;
    sqlx::query(
        r#"
        INSERT INTO registration_gate_webhook_deliveries (
            endpoint_id, event_id, kind, payload, status, attempt_count, cycle_attempt_count,
            cycle_started_at, next_attempt_at, last_result, last_http_status, failure_reason,
            failed_at, created_at, updated_at
        ) VALUES ($1, $2, 'outage_started', '{}', 'failed', 8, 8, $3, $3, 'http_server_error',
                  500, 'retries_exhausted', $4, $3, $3)
        "#,
    )
    .bind(endpoint_id)
    .bind(event_id)
    .bind(failed_at - TimeDuration::hours(1))
    .bind(failed_at)
    .execute(con)
    .await
    .unwrap();
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn each_endpoint_gets_the_same_event_ids_signed_with_its_own_secret() {
    let fx = Fixture::new().await;
    let (server_a, server_b) = (MockServer::start().await, MockServer::start().await);
    mount_ok(&server_a).await;
    mount_ok(&server_b).await;
    let cipher = cipher_1();
    let sender = endpoint_sender();
    let all = WebhookCategory::ALL;
    add_endpoint(
        &fx,
        &cipher,
        &format!("{}/a", server_a.uri()),
        SECRET_A,
        &all,
    )
    .await;
    add_endpoint(
        &fx,
        &cipher,
        &format!("{}/b", server_b.uri()),
        SECRET_B,
        &all,
    )
    .await;

    fx.emit_all_kinds().await;
    let events = fx.event_ids_and_kinds().await;
    assert!(events.len() >= 8, "all event kinds are produced");
    let t0 = now();
    let policy = default_policy();
    for _ in 0..3 {
        run_cycle(&fx, &sender, &cipher, &policy, t0).await;
    }

    let (a, b) = (received(&server_a).await, received(&server_b).await);
    let expected: BTreeSet<String> = events.iter().map(|(id, _)| id.to_string()).collect();
    assert_eq!(
        event_id_set(&a),
        expected,
        "endpoint A got every event once"
    );
    assert_eq!(
        event_id_set(&b),
        expected,
        "endpoint B got every event once"
    );
    assert_eq!(a.len(), expected.len());
    assert_eq!(b.len(), expected.len());
    let now_unix = t0.unix_timestamp();
    for (received, secret, other) in [(&a, SECRET_A, SECRET_B), (&b, SECRET_B, SECRET_A)] {
        for request in received {
            signature::verify(
                secret.as_bytes(),
                &request.signature,
                request.timestamp,
                &request.body,
                now_unix,
                600,
            )
            .expect("signed with this endpoint's secret");
            assert!(
                signature::verify(
                    other.as_bytes(),
                    &request.signature,
                    request.timestamp,
                    &request.body,
                    now_unix,
                    600,
                )
                .is_err(),
                "the other endpoint's secret must not verify"
            );
        }
    }
    for request_a in &a {
        let request_b = b.iter().find(|r| r.event_id == request_a.event_id).unwrap();
        assert_eq!(request_a.body, request_b.body);
        assert_ne!(request_a.signature, request_b.signature);
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_slow_or_failing_endpoint_never_blocks_or_changes_the_others_or_the_gate() {
    let fx = Fixture::new().await;
    let (slow, failing, healthy) = (
        MockServer::start().await,
        MockServer::start().await,
        MockServer::start().await,
    );
    Mock::given(any())
        .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(3)))
        .mount(&slow)
        .await;
    mount_status(&failing, 500).await;
    mount_ok(&healthy).await;
    let cipher = cipher_1();
    let sender = endpoint_sender();
    let all = WebhookCategory::ALL;
    let slow_endpoint =
        add_endpoint(&fx, &cipher, &format!("{}/s", slow.uri()), SECRET_A, &all).await;
    let failing_endpoint = add_endpoint(
        &fx,
        &cipher,
        &format!("{}/f", failing.uri()),
        SECRET_A,
        &all,
    )
    .await;
    let healthy_endpoint = add_endpoint(
        &fx,
        &cipher,
        &format!("{}/h", healthy.uri()),
        SECRET_B,
        &all,
    )
    .await;

    fx.emit_all_kinds().await;
    let gate_before = fx.gate_state().await;
    let events_before = fx.event_ids_and_kinds().await;
    let audit_before = fx.count("registration_gate_audit").await;
    let t0 = now();
    run_cycle(&fx, &sender, &cipher, &default_policy(), t0).await;

    let delivered = deliveries_of(&fx, healthy_endpoint.id).await;
    assert_eq!(delivered.len(), events_before.len());
    assert!(delivered
        .iter()
        .all(|d| d.status == WebhookDeliveryStatus::Delivered));
    for stuck in [slow_endpoint.id, failing_endpoint.id] {
        let rows = deliveries_of(&fx, stuck).await;
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|d| matches!(
            d.status,
            WebhookDeliveryStatus::Retrying | WebhookDeliveryStatus::Pending
        )));
    }
    assert_eq!(fx.gate_state().await, gate_before);
    assert_eq!(fx.event_ids_and_kinds().await, events_before);
    assert_eq!(fx.count("registration_gate_audit").await, audit_before);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn only_events_of_the_selected_categories_are_delivered() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_ok(&server).await;
    let cipher = cipher_1();
    let sender = endpoint_sender();
    let endpoint = add_endpoint(
        &fx,
        &cipher,
        &format!("{}/c", server.uri()),
        SECRET_A,
        &[WebhookCategory::Outage, WebhookCategory::Override],
    )
    .await;
    fx.emit_all_kinds().await;
    let events = fx.event_ids_and_kinds().await;
    let expected: BTreeSet<String> = events
        .iter()
        .filter(|(_, kind)| {
            let kind: GateEventKind =
                serde_json::from_value(serde_json::Value::String(kind.clone())).unwrap();
            matches!(
                category_of(kind),
                WebhookCategory::Outage | WebhookCategory::Override
            )
        })
        .map(|(id, _)| id.to_string())
        .collect();
    assert!(!expected.is_empty() && expected.len() < events.len());
    for _ in 0..3 {
        run_cycle(&fx, &sender, &cipher, &default_policy(), now()).await;
    }
    assert_eq!(event_id_set(&received(&server).await), expected);
    assert_eq!(deliveries_of(&fx, endpoint.id).await.len(), expected.len());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_category_change_only_affects_events_after_the_change() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_ok(&server).await;
    let cipher = cipher_1();
    let sender = endpoint_sender();
    let endpoint = add_endpoint(
        &fx,
        &cipher,
        &format!("{}/c", server.uri()),
        SECRET_A,
        &[WebhookCategory::Outage],
    )
    .await;
    fx.command(ACK).await;
    fx.command(GateCommand::RevokeOverride).await;
    fx.observe(obs(H, OFF)).await;
    fx.command(GateCommand::Open).await;
    fx.observe(obs(DOWN, OFF)).await;
    let before_change = fx.event_ids_and_kinds().await;
    let outage_before: BTreeSet<String> = before_change
        .iter()
        .filter(|(_, kind)| kind == "outage_started" || kind == "protection_degraded")
        .map(|(id, _)| id.to_string())
        .collect();
    assert!(!outage_before.is_empty());

    {
        let mut conn = fx.db.connection().await.unwrap();
        let outcome = PostgresWebhookDeliveryRepository
            .update_endpoint(
                &mut conn,
                endpoint.id,
                endpoint.revision,
                WebhookEndpointPatch {
                    categories: WebhookCategories::new([WebhookCategory::Override]),
                    ..Default::default()
                },
                &admin(),
                now(),
            )
            .await
            .unwrap();
        let WebhookEndpointWrite::Done(updated) = outcome else {
            panic!("update failed: {outcome:?}")
        };
        assert_eq!(updated.revision, endpoint.revision + 1);
    }
    for _ in 0..3 {
        run_cycle(&fx, &sender, &cipher, &default_policy(), now()).await;
    }
    assert_eq!(event_id_set(&received(&server).await), outage_before);

    let known: BTreeSet<i64> = before_change.iter().map(|(id, _)| *id).collect();
    fx.observe(obs(H, OFF)).await;
    fx.command(ACK).await;
    fx.command(GateCommand::RevokeOverride).await;
    let after = fx.event_ids_and_kinds().await;
    let new_override: BTreeSet<String> = after
        .iter()
        .filter(|(id, kind)| {
            !known.contains(id) && (kind == "override_started" || kind == "override_revoked")
        })
        .map(|(id, _)| id.to_string())
        .collect();
    assert!(!new_override.is_empty());
    for _ in 0..3 {
        run_cycle(&fx, &sender, &cipher, &default_policy(), now()).await;
    }
    let got = event_id_set(&received(&server).await);
    assert!(new_override.is_subset(&got));
    assert!(
        got.difference(&outage_before)
            .all(|id| new_override.contains(id)),
        "no outage/recovery event after the change reaches the endpoint"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_new_endpoint_never_receives_events_from_before_its_creation() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_ok(&server).await;
    let cipher = cipher_1();
    let sender = endpoint_sender();
    fx.emit_all_kinds().await;
    let old: BTreeSet<i64> = fx
        .event_ids_and_kinds()
        .await
        .iter()
        .map(|(id, _)| *id)
        .collect();
    let endpoint = add_endpoint(
        &fx,
        &cipher,
        &format!("{}/n", server.uri()),
        SECRET_A,
        &WebhookCategory::ALL,
    )
    .await;
    for _ in 0..2 {
        run_cycle(&fx, &sender, &cipher, &default_policy(), now()).await;
    }
    assert!(received(&server).await.is_empty());
    assert!(deliveries_of(&fx, endpoint.id).await.is_empty());

    fx.command(ACK).await;
    for _ in 0..2 {
        run_cycle(&fx, &sender, &cipher, &default_policy(), now()).await;
    }
    let got = received(&server).await;
    assert_eq!(got.len(), 1);
    assert!(!old.contains(&got[0].event_id.parse::<i64>().unwrap()));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn deleting_an_endpoint_drops_its_unfinished_deliveries_and_stops_delivery() {
    let fx = Fixture::new().await;
    let (server_a, server_b) = (MockServer::start().await, MockServer::start().await);
    mount_status(&server_a, 500).await;
    mount_ok(&server_b).await;
    let cipher = cipher_1();
    let sender = endpoint_sender();
    let all = WebhookCategory::ALL;
    let a = add_endpoint(
        &fx,
        &cipher,
        &format!("{}/a", server_a.uri()),
        SECRET_A,
        &all,
    )
    .await;
    let b = add_endpoint(
        &fx,
        &cipher,
        &format!("{}/b", server_b.uri()),
        SECRET_B,
        &all,
    )
    .await;
    fx.command(ACK).await;
    let first_events = fx.event_ids_and_kinds().await.len();
    run_cycle(&fx, &sender, &cipher, &default_policy(), now()).await;
    assert_eq!(deliveries_of(&fx, a.id).await.len(), first_events);

    let mut conn = fx.db.connection().await.unwrap();
    let outcome = PostgresWebhookDeliveryRepository
        .delete_endpoint(&mut conn, a.id, a.revision + 5, &admin(), now())
        .await
        .unwrap();
    assert_eq!(outcome, WebhookEndpointWrite::RevisionConflict);
    let outcome = PostgresWebhookDeliveryRepository
        .delete_endpoint(&mut conn, a.id, a.revision, &admin(), now())
        .await
        .unwrap();
    assert!(matches!(outcome, WebhookEndpointWrite::Done(_)));
    assert!(deliveries_of(&fx, a.id).await.is_empty());
    assert_eq!(
        fx.count("registration_gate_webhook_attempts").await,
        first_events as i64,
        "only B's attempts remain"
    );
    let again = PostgresWebhookDeliveryRepository
        .delete_endpoint(&mut conn, a.id, a.revision, &admin(), now())
        .await
        .unwrap();
    assert_eq!(again, WebhookEndpointWrite::NotFound);
    drop(conn);

    let hits_before = server_a.received_requests().await.unwrap().len();
    fx.command(GateCommand::RevokeOverride).await;
    for _ in 0..2 {
        run_cycle(
            &fx,
            &sender,
            &cipher,
            &default_policy(),
            now() + TimeDuration::hours(1),
        )
        .await;
    }
    assert_eq!(
        server_a.received_requests().await.unwrap().len(),
        hits_before
    );
    let all_events = fx.event_ids_and_kinds().await.len();
    assert!(all_events > first_events);
    assert_eq!(received(&server_b).await.len(), all_events);
    assert_eq!(deliveries_of(&fx, b.id).await.len(), all_events);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn crud_enforces_revision_duplicate_url_and_the_endpoint_limit_and_audits_each_write() {
    let fx = Fixture::new().await;
    let cipher = cipher_1();
    let first = add_endpoint(
        &fx,
        &cipher,
        "https://one.example.net/hook/token-one-0123456789",
        SECRET_A,
        &[WebhookCategory::Outage],
    )
    .await;
    assert_eq!(first.revision, 1);
    assert_eq!(first.source, WebhookEndpointSource::Api);

    let mut conn = fx.db.connection().await.unwrap();
    let repository = PostgresWebhookDeliveryRepository;
    let duplicate = NewWebhookEndpoint {
        id: WebhookEndpointId::generate(),
        url: ValidatedWebhookUrl::new_checked(first.url.clone()),
        sealed_secret: first.sealed_secret.clone(),
        categories: WebhookCategories::all(),
        source: WebhookEndpointSource::Api,
    };
    assert_eq!(
        repository
            .create_endpoint(&mut conn, duplicate, &admin(), now())
            .await
            .unwrap(),
        WebhookEndpointWrite::DuplicateUrl
    );

    let stale = repository
        .update_endpoint(
            &mut conn,
            first.id,
            99,
            WebhookEndpointPatch {
                categories: WebhookCategories::new([WebhookCategory::Recovery]),
                ..Default::default()
            },
            &admin(),
            now(),
        )
        .await
        .unwrap();
    assert_eq!(stale, WebhookEndpointWrite::RevisionConflict);
    let unchanged = repository
        .get_endpoint(&mut conn, first.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.revision, 1);
    assert_eq!(unchanged.categories, first.categories);

    let WebhookEndpointWrite::Done(updated) = repository
        .update_endpoint(
            &mut conn,
            first.id,
            1,
            WebhookEndpointPatch {
                categories: WebhookCategories::new([WebhookCategory::Recovery]),
                ..Default::default()
            },
            &admin(),
            now(),
        )
        .await
        .unwrap()
    else {
        panic!("update")
    };
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.url, first.url, "an omitted URL is kept");
    assert_eq!(
        updated.sealed_secret, first.sealed_secret,
        "an omitted secret is kept"
    );
    assert_eq!(updated.categories.as_slice(), [WebhookCategory::Recovery]);

    let second = add_endpoint(
        &fx,
        &cipher,
        "https://two.example.net/hook/token-two-0123456789",
        SECRET_B,
        &WebhookCategory::ALL,
    )
    .await;
    let clash = repository
        .update_endpoint(
            &mut conn,
            second.id,
            1,
            WebhookEndpointPatch {
                url: Some(ValidatedWebhookUrl::new_checked(first.url.clone())),
                ..Default::default()
            },
            &admin(),
            now(),
        )
        .await
        .unwrap();
    assert_eq!(clash, WebhookEndpointWrite::DuplicateUrl);
    assert_eq!(
        repository
            .get_endpoint(&mut conn, second.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        repository
            .update_endpoint(
                &mut conn,
                WebhookEndpointId::new(1),
                1,
                WebhookEndpointPatch::default(),
                &admin(),
                now(),
            )
            .await
            .unwrap(),
        WebhookEndpointWrite::NotFound
    );

    for index in 2..MAX_WEBHOOK_ENDPOINTS {
        add_endpoint(
            &fx,
            &cipher,
            &format!("https://limit-{index}.example.net/hook"),
            SECRET_A,
            &WebhookCategory::ALL,
        )
        .await;
    }
    assert_eq!(
        repository.list_endpoints(&mut conn).await.unwrap().len() as i64,
        MAX_WEBHOOK_ENDPOINTS
    );
    let over = NewWebhookEndpoint {
        id: WebhookEndpointId::generate(),
        url: ValidatedWebhookUrl::new_checked("https://over.example.net/hook".to_string()),
        sealed_secret: first.sealed_secret.clone(),
        categories: WebhookCategories::all(),
        source: WebhookEndpointSource::Api,
    };
    assert_eq!(
        repository
            .create_endpoint(&mut conn, over, &admin(), now())
            .await
            .unwrap(),
        WebhookEndpointWrite::LimitReached
    );

    let con: &mut PgConnection = &mut conn;
    type AuditRow = (Option<i64>, String, String, Option<i64>, Option<String>);
    let audit: Vec<AuditRow> = sqlx::query_as(
        "SELECT actor, operation, outcome, revision, detail \
         FROM registration_gate_webhook_endpoint_audit WHERE endpoint_id = $1 ORDER BY id",
    )
    .bind(first.id.as_i64())
    .fetch_all(&mut *con)
    .await
    .unwrap();
    assert_eq!(
        audit
            .iter()
            .map(|row| (row.1.as_str(), row.2.as_str(), row.3))
            .collect::<Vec<_>>(),
        [
            ("create", "succeeded", Some(1)),
            ("update", "succeeded", Some(2))
        ]
    );
    assert!(audit.iter().all(|row| row.0.is_some() && row.4.is_none()));
    repository
        .record_audit(
            &mut conn,
            &WebhookAuditEntry {
                actor: Some(admin()),
                endpoint_id: Some(first.id),
                operation: WebhookAuditOperation::Test,
                outcome: WebhookAuditOutcome::DeliveryFailed,
                revision: None,
                detail: Some("http_server_error".to_string()),
            },
            now(),
        )
        .await
        .unwrap();
    let con: &mut PgConnection = &mut conn;
    let dump: String = sqlx::query_scalar(
        "SELECT COALESCE(string_agg(row_to_json(a)::text, ' '), '') \
         FROM registration_gate_webhook_endpoint_audit a",
    )
    .fetch_one(con)
    .await
    .unwrap();
    for forbidden in [
        SECRET_A,
        SECRET_B,
        "one.example.net",
        "token-one",
        "https://",
    ] {
        assert!(!dump.contains(forbidden), "audit leaks {forbidden}");
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn only_ciphertext_of_the_secret_is_persisted_anywhere() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 500).await;
    let cipher = cipher_1();
    let sender = endpoint_sender();
    let url = format!("{}/token-0123456789abcdef", server.uri());
    let endpoint = add_endpoint(&fx, &cipher, &url, SECRET_A, &WebhookCategory::ALL).await;
    fx.command(ACK).await;
    run_cycle(&fx, &sender, &cipher, &default_policy(), now()).await;
    assert!(!deliveries_of(&fx, endpoint.id).await.is_empty());

    let mut conn = fx.db.connection().await.unwrap();
    let con: &mut PgConnection = &mut conn;
    for table in [
        "registration_gate_webhook_endpoints",
        "registration_gate_webhook_endpoint_audit",
        "registration_gate_webhook_deliveries",
        "registration_gate_webhook_attempts",
        "registration_gate_webhook_state",
        "registration_gate_webhook_env_import",
    ] {
        let dump: String = sqlx::query_scalar(&format!(
            "SELECT COALESCE(string_agg(row_to_json(t)::text, ' '), '') FROM {table} t"
        ))
        .fetch_one(&mut *con)
        .await
        .unwrap();
        assert!(
            !dump.contains(SECRET_A),
            "{table} contains the plaintext secret"
        );
    }
    let (key_id, ciphertext): (String, Vec<u8>) = sqlx::query_as(
        "SELECT secret_key_id, secret_ciphertext FROM registration_gate_webhook_endpoints",
    )
    .fetch_one(&mut *con)
    .await
    .unwrap();
    assert!(key_id.starts_with("kf-"));
    assert!(!ciphertext
        .windows(SECRET_A.len())
        .any(|window| window == SECRET_A.as_bytes()));
    assert_eq!(
        cipher
            .open(endpoint.id, &endpoint.sealed_secret)
            .unwrap()
            .expose_bytes(),
        SECRET_A.as_bytes()
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_undecryptable_secret_fails_only_that_endpoint_and_recovers_after_reentry() {
    let fx = Fixture::new().await;
    let (server_a, server_b) = (MockServer::start().await, MockServer::start().await);
    mount_ok(&server_a).await;
    mount_ok(&server_b).await;
    let sender = endpoint_sender();
    let all = WebhookCategory::ALL;
    let lost = add_endpoint(
        &fx,
        &cipher_1(),
        &format!("{}/a", server_a.uri()),
        SECRET_A,
        &all,
    )
    .await;
    let healthy = add_endpoint(
        &fx,
        &cipher_2(),
        &format!("{}/b", server_b.uri()),
        SECRET_B,
        &all,
    )
    .await;
    fx.command(ACK).await;
    let gate_before = fx.gate_state().await;
    let events_before = fx.event_ids_and_kinds().await;

    let report = run_cycle(&fx, &sender, &cipher_2(), &default_policy(), now()).await;
    assert_eq!(report.failed, 1);
    assert_eq!(report.delivered, 1);
    assert!(
        received(&server_a).await.is_empty(),
        "nothing is sent without a usable secret"
    );
    let lost_rows = deliveries_of(&fx, lost.id).await;
    assert_eq!(lost_rows.len(), 1);
    assert_eq!(lost_rows[0].status, WebhookDeliveryStatus::Failed);
    assert_eq!(
        lost_rows[0].failure_reason,
        Some(WebhookFailureReason::PermanentError)
    );
    assert_eq!(
        lost_rows[0].last_result,
        Some(WebhookResultClass::SecretUndecryptable)
    );
    assert_eq!(
        deliveries_of(&fx, healthy.id).await[0].status,
        WebhookDeliveryStatus::Delivered
    );
    assert_eq!(fx.gate_state().await, gate_before);
    assert_eq!(fx.event_ids_and_kinds().await, events_before);

    {
        let mut conn = fx.db.connection().await.unwrap();
        let sealed = cipher_2()
            .seal(lost.id, &WebhookSecret::parse(SECRET_A).unwrap())
            .unwrap();
        let outcome = PostgresWebhookDeliveryRepository
            .update_endpoint(
                &mut conn,
                lost.id,
                lost.revision,
                WebhookEndpointPatch {
                    sealed_secret: Some(sealed),
                    ..Default::default()
                },
                &admin(),
                now(),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, WebhookEndpointWrite::Done(_)));
    }
    assert_eq!(fx.retry_failed(None, now()).await, 1);
    let report = run_cycle(&fx, &sender, &cipher_2(), &default_policy(), now()).await;
    assert_eq!(report.delivered, 1);
    let got = received(&server_a).await;
    assert_eq!(got.len(), 1);
    signature::verify(
        SECRET_A.as_bytes(),
        &got[0].signature,
        got[0].timestamp,
        &got[0].body,
        now().unix_timestamp(),
        600,
    )
    .unwrap();
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn retry_requeues_one_delivery_of_one_endpoint_or_all_of_an_endpoint_or_everything() {
    let fx = Fixture::new().await;
    let cipher = cipher_1();
    let a = add_endpoint(
        &fx,
        &cipher,
        "https://a.example.net/h",
        SECRET_A,
        &WebhookCategory::ALL,
    )
    .await;
    let b = add_endpoint(
        &fx,
        &cipher,
        "https://b.example.net/h",
        SECRET_B,
        &WebhookCategory::ALL,
    )
    .await;
    let t0 = now();
    for event in [1001, 1002, 1003] {
        insert_failed(&fx, a.id.as_i64(), event, t0).await;
    }
    for event in [1001, 1002] {
        insert_failed(&fx, b.id.as_i64(), event, t0).await;
    }
    let mut conn = fx.db.connection().await.unwrap();
    let repository = PostgresWebhookDeliveryRepository;
    let id = RegistrationGateEventId::new;

    assert_eq!(
        repository
            .retry_failed(&mut conn, Some(a.id), Some(&id(1001)), t0)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repository
            .retry_failed(&mut conn, Some(a.id), Some(&id(1001)), t0)
            .await
            .unwrap(),
        0,
        "no longer failed"
    );
    assert_eq!(
        repository
            .retry_failed(&mut conn, Some(b.id), Some(&id(1003)), t0)
            .await
            .unwrap(),
        0,
        "B never had 1003"
    );
    let pending_a: Vec<_> = deliveries_of(&fx, a.id)
        .await
        .into_iter()
        .filter(|d| d.status == WebhookDeliveryStatus::Pending)
        .collect();
    assert_eq!(pending_a.len(), 1);
    assert_eq!(*pending_a[0].event_id.as_ref(), 1001);
    assert!(deliveries_of(&fx, b.id)
        .await
        .iter()
        .all(|d| d.status == WebhookDeliveryStatus::Failed));

    assert_eq!(
        repository
            .retry_failed(&mut conn, Some(a.id), None, t0)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        deliveries_of(&fx, b.id)
            .await
            .iter()
            .filter(|d| d.status == WebhookDeliveryStatus::Failed)
            .count(),
        2
    );
    assert_eq!(
        repository
            .retry_failed(&mut conn, None, None, t0)
            .await
            .unwrap(),
        2
    );
    let stats = repository.endpoint_stats(&mut conn).await.unwrap();
    assert!(stats.iter().all(|(_, stats)| stats.counts.failed == 0));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn failed_deliveries_page_through_more_than_fifty_and_the_oldest_can_be_retried_alone() {
    let fx = Fixture::new().await;
    let cipher = cipher_1();
    let a = add_endpoint(
        &fx,
        &cipher,
        "https://a.example.net/h",
        SECRET_A,
        &WebhookCategory::ALL,
    )
    .await;
    let b = add_endpoint(
        &fx,
        &cipher,
        "https://b.example.net/h",
        SECRET_B,
        &WebhookCategory::ALL,
    )
    .await;
    let t0 = now();
    for index in 0..60i64 {
        insert_failed(
            &fx,
            a.id.as_i64(),
            5000 + index,
            t0 + TimeDuration::minutes(index / 10),
        )
        .await;
    }
    for index in 0..5i64 {
        insert_failed(
            &fx,
            b.id.as_i64(),
            9000 + index,
            t0 + TimeDuration::minutes(index),
        )
        .await;
    }
    {
        let mut conn = fx.db.connection().await.unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query(
            r#"INSERT INTO registration_gate_webhook_deliveries (
                endpoint_id, event_id, kind, payload, status, attempt_count, cycle_attempt_count,
                cycle_started_at, next_attempt_at, delivered_at, created_at, updated_at)
               VALUES ($1, 7777, 'outage_started', '{}', 'delivered', 1, 1, $2, $2, $2, $2, $2)"#,
        )
        .bind(a.id.as_i64())
        .bind(t0)
        .execute(con)
        .await
        .unwrap();
    }

    let repository = PostgresWebhookDeliveryRepository;
    let mut conn = fx.db.connection().await.unwrap();
    let mut seen = Vec::new();
    let mut before: Option<WebhookFailureCursor> = None;
    let mut pages = 0;
    loop {
        let page = kernel::prelude::entity::build_failed_page(
            repository
                .list_failed(&mut conn, None, before.as_ref(), 21)
                .await
                .unwrap(),
            20,
        );
        pages += 1;
        seen.extend(page.items.iter().cloned());
        match page.next_cursor {
            Some(cursor) => before = Some(WebhookFailureCursor::decode(&cursor).unwrap()),
            None => break,
        }
    }
    assert_eq!(seen.len(), 65, "every failed delivery is reachable");
    assert_eq!(pages, 4);
    let keys: Vec<_> = seen
        .iter()
        .map(|d| (d.failed_at, d.endpoint_id, *d.event_id.as_ref()))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_by(|x, y| y.cmp(x));
    assert_eq!(keys, sorted, "stable newest-first order");
    assert_eq!(
        keys.iter().collect::<BTreeSet<_>>().len(),
        keys.len(),
        "no duplicates across pages"
    );
    assert!(seen.iter().all(|d| *d.event_id.as_ref() != 7777));

    let only_a = repository
        .list_failed(&mut conn, Some(a.id), None, 1000)
        .await
        .unwrap();
    assert_eq!(only_a.len(), 60);
    assert!(only_a.iter().all(|d| d.endpoint_id == a.id));

    let recent_window = &only_a[..50];
    assert!(
        recent_window.iter().all(|d| *d.event_id.as_ref() != 5000),
        "the oldest failure is outside the most recent 50"
    );
    let oldest = seen
        .iter()
        .find(|d| d.endpoint_id == a.id && *d.event_id.as_ref() == 5000)
        .expect("reachable through paging");
    assert_eq!(
        repository
            .retry_failed(
                &mut conn,
                Some(oldest.endpoint_id),
                Some(&oldest.event_id),
                now()
            )
            .await
            .unwrap(),
        1
    );
    let after = repository
        .list_failed(&mut conn, Some(a.id), None, 1000)
        .await
        .unwrap();
    assert_eq!(after.len(), 59);
    assert!(after.iter().all(|d| *d.event_id.as_ref() != 5000));
    let status = deliveries_of(&fx, a.id)
        .await
        .into_iter()
        .find(|d| *d.event_id.as_ref() == 5000)
        .unwrap();
    assert_eq!(status.status, WebhookDeliveryStatus::Pending);
    assert_eq!(status.failure_reason, None);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn failed_counts_follow_the_failed_deliveries_and_drop_after_they_are_resolved() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 400).await;
    let cipher = cipher_1();
    let sender = endpoint_sender();
    let endpoint = add_endpoint(
        &fx,
        &cipher,
        &format!("{}/x", server.uri()),
        SECRET_A,
        &WebhookCategory::ALL,
    )
    .await;
    fx.command(ACK).await;
    run_cycle(&fx, &sender, &cipher, &default_policy(), now()).await;
    let failed_count = |stats: &[(
        WebhookEndpointId,
        kernel::prelude::entity::WebhookEndpointStats,
    )]| {
        stats
            .iter()
            .find(|(id, _)| *id == endpoint.id)
            .map(|(_, stats)| stats.counts.failed)
            .unwrap_or(0)
    };
    let repository = PostgresWebhookDeliveryRepository;
    let mut conn = fx.db.connection().await.unwrap();
    assert_eq!(
        failed_count(&repository.endpoint_stats(&mut conn).await.unwrap()),
        1
    );

    server.reset().await;
    mount_ok(&server).await;
    assert_eq!(
        repository
            .retry_failed(&mut conn, Some(endpoint.id), None, now())
            .await
            .unwrap(),
        1
    );
    drop(conn);
    run_cycle(&fx, &sender, &cipher, &default_policy(), now()).await;
    let mut conn = fx.db.connection().await.unwrap();
    let stats = repository.endpoint_stats(&mut conn).await.unwrap();
    assert_eq!(failed_count(&stats), 0, "resolved failures no longer count");
    assert!(repository
        .list_failed(&mut conn, Some(endpoint.id), None, 10)
        .await
        .unwrap()
        .is_empty());
}

fn import_request(cipher: &AeadWebhookSecretCipher, url: &str) -> NewWebhookEndpoint {
    let id = WebhookEndpointId::generate();
    NewWebhookEndpoint {
        id,
        url: ValidatedWebhookUrl::new_checked(url.to_string()),
        sealed_secret: cipher
            .seal(id, &WebhookSecret::parse(SECRET).unwrap())
            .unwrap(),
        categories: WebhookCategories::all(),
        source: WebhookEndpointSource::EnvironmentImport,
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn importing_the_environment_destination_keeps_baseline_and_records_and_is_idempotent() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 400).await;
    let env_sender = fx.sender(&server);
    let policy = default_policy();
    let t0 = now();
    fx.cycle(&env_sender, "w", &policy, t0).await;
    fx.command(ACK).await;
    fx.cycle(&env_sender, "w", &policy, t0).await;
    fx.command(GateCommand::RevokeOverride).await;
    let before = fx.deliveries().await;
    assert_eq!(
        before.len(),
        1,
        "the second event is still waiting to be ingested"
    );
    assert!(before
        .iter()
        .all(|d| d.endpoint_id == WebhookEndpointId::ENVIRONMENT));
    let failed_event = *before[0].event_id.as_ref();
    let attempts_before = fx.attempts(failed_event).await;
    assert_eq!(attempts_before.len(), 1);
    let baseline: i64 = {
        let mut conn = fx.db.connection().await.unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query_scalar("SELECT baseline_event_id FROM registration_gate_webhook_state")
            .fetch_one(con)
            .await
            .unwrap()
    };

    let cipher = cipher_1();
    let repository = PostgresWebhookDeliveryRepository;
    let mut conn = fx.db.connection().await.unwrap();
    assert!(!repository.environment_import_done(&mut conn).await.unwrap());
    let request = import_request(&cipher, &format!("{}/hook", server.uri()));
    let imported_id = request.id;
    let imported = repository
        .import_environment_endpoint(&mut conn, request, now())
        .await
        .unwrap()
        .expect("imported");
    assert_eq!(imported.id, imported_id);
    assert_eq!(
        imported.baseline_event_id, baseline,
        "the E15w baseline is carried over"
    );
    assert_eq!(imported.categories, WebhookCategories::all());
    assert_eq!(imported.source, WebhookEndpointSource::EnvironmentImport);
    assert!(repository.environment_import_done(&mut conn).await.unwrap());
    drop(conn);

    let after = deliveries_of(&fx, imported.id).await;
    assert_eq!(after.len(), 1);
    assert_eq!(*after[0].event_id.as_ref(), failed_event);
    assert_eq!(after[0].status, WebhookDeliveryStatus::Failed);
    let mut conn = fx.db.connection().await.unwrap();
    assert_eq!(
        repository
            .list_attempts(
                &mut conn,
                imported.id,
                &RegistrationGateEventId::new(failed_event)
            )
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(deliveries_of(&fx, WebhookEndpointId::ENVIRONMENT)
        .await
        .is_empty());

    let again = repository
        .import_environment_endpoint(
            &mut conn,
            import_request(&cipher, &format!("{}/hook2", server.uri())),
            now(),
        )
        .await
        .unwrap();
    assert!(again.is_none());
    assert_eq!(repository.list_endpoints(&mut conn).await.unwrap().len(), 1);

    assert!(matches!(
        repository
            .delete_endpoint(&mut conn, imported.id, imported.revision, &admin(), now())
            .await
            .unwrap(),
        WebhookEndpointWrite::Done(_)
    ));
    let reimport = repository
        .import_environment_endpoint(
            &mut conn,
            import_request(&cipher, &format!("{}/hook", server.uri())),
            now(),
        )
        .await
        .unwrap();
    assert!(reimport.is_none());
    assert!(repository
        .list_endpoints(&mut conn)
        .await
        .unwrap()
        .is_empty());
    assert!(repository.environment_import_done(&mut conn).await.unwrap());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn without_an_encryption_key_the_environment_destination_keeps_delivering_as_before() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_ok(&server).await;
    let env_sender = fx.sender(&server);
    let policy = default_policy();
    fx.cycle(&env_sender, "w", &policy, now()).await;
    fx.command(ACK).await;
    let first_events = fx.event_ids_and_kinds().await.len();
    let report = fx.cycle(&env_sender, "w", &policy, now()).await;
    assert_eq!(report.delivered as usize, first_events);
    assert_eq!(received(&server).await.len(), first_events);
    let cipher = AeadWebhookSecretCipher::not_configured();
    assert!(!cipher.is_configured());
    fx.command(GateCommand::RevokeOverride).await;
    let all_events = fx.event_ids_and_kinds().await.len();
    let report = run_cycle(&fx, &env_sender, &cipher, &policy, now()).await;
    assert_eq!(report.delivered as usize, all_events - first_events);
    assert_eq!(received(&server).await.len(), all_events);
    assert_eq!(fx.count("registration_gate_webhook_endpoints").await, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_endpoint_that_now_resolves_to_an_internal_address_is_blocked_at_connect_time() {
    let fx = Fixture::new().await;
    let cipher = cipher_1();
    let endpoint = add_endpoint(
        &fx,
        &cipher,
        "https://rebinding.example.net/hook",
        SECRET_A,
        &WebhookCategory::ALL,
    )
    .await;
    struct Internal;
    impl HostResolver for Internal {
        fn resolve<'a>(
            &'a self,
            _host: &'a str,
            port: u16,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + 'a>,
        > {
            Box::pin(async move { Ok(vec![SocketAddr::from(([169, 254, 169, 254], port))]) })
        }
    }
    let sender = HttpWebhookSender::disabled()
        .with_settings(fast_settings())
        .with_resolver(Arc::new(Internal));
    fx.command(ACK).await;
    let report = run_cycle(&fx, &sender, &cipher, &default_policy(), now()).await;
    assert_eq!(report.failed, 1);
    let row = &deliveries_of(&fx, endpoint.id).await[0];
    assert_eq!(row.status, WebhookDeliveryStatus::Failed);
    assert_eq!(row.last_result, Some(WebhookResultClass::SsrfBlocked));
    assert_eq!(
        row.failure_reason,
        Some(WebhookFailureReason::PermanentError)
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn the_migration_keeps_every_legacy_delivery_record_and_event_id() {
    use sqlx::migrate::Migrator;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use sqlx::Connection as _;

    let url = std::env::var("DATABASE_URL").unwrap();
    let options: PgConnectOptions = url.parse().unwrap();
    let database = format!(
        "e15x_migration_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let mut admin = PgConnection::connect_with(&options).await.unwrap();
    admin
        .execute(format!("CREATE DATABASE {database}").as_str())
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone().database(&database))
        .await
        .unwrap();

    let all = sqlx::migrate!("../migrations");
    let new_version = all
        .migrations
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap();
    let before: Vec<_> = all
        .migrations
        .iter()
        .filter(|migration| migration.version < new_version)
        .cloned()
        .collect();
    assert!(before.len() + 1 == all.migrations.len());
    Migrator {
        migrations: std::borrow::Cow::Owned(before),
        ignore_missing: false,
        locking: true,
    }
    .run(&pool)
    .await
    .unwrap();

    let t = now();
    pool.execute(
        sqlx::query(
            "INSERT INTO registration_gate_webhook_state (id, baseline_event_id, enabled_since) \
             VALUES (1, 41, $1)",
        )
        .bind(t),
    )
    .await
    .unwrap();
    for (event_id, status, last_result, failure_reason, delivered_at) in [
        (101i64, "pending", None, None, None),
        (102, "retrying", Some("http_server_error"), None, None),
        (103, "delivered", Some("delivered"), None, Some(t)),
        (
            104,
            "failed",
            Some("http_client_error"),
            Some("permanent_error"),
            None,
        ),
    ] {
        pool.execute(
            sqlx::query(
                r#"INSERT INTO registration_gate_webhook_deliveries (
                    event_id, kind, payload, status, attempt_count, cycle_attempt_count,
                    cycle_started_at, next_attempt_at, last_result, failure_reason, delivered_at,
                    created_at, updated_at)
                   VALUES ($1, 'outage_started', '{"legacy":true}', $2, 1, 1, $3, $3, $4, $5, $6, $3, $3)"#,
            )
            .bind(event_id)
            .bind(status)
            .bind(t)
            .bind(last_result)
            .bind(failure_reason)
            .bind(delivered_at),
        )
        .await
        .unwrap();
        pool.execute(
            sqlx::query(
                r#"INSERT INTO registration_gate_webhook_attempts
                   (event_id, attempt_no, started_at, finished_at, result_class, http_status)
                   VALUES ($1, 1, $2, $2, 'http_server_error', 500)"#,
            )
            .bind(event_id)
            .bind(t),
        )
        .await
        .unwrap();
    }

    all.run(&pool).await.unwrap();

    let rows: Vec<(i64, i64, String, String, bool)> = sqlx::query_as(
        "SELECT event_id, endpoint_id, status, payload, failed_at IS NOT NULL \
         FROM registration_gate_webhook_deliveries ORDER BY event_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let legacy = "{\"legacy\":true}".to_string();
    assert_eq!(
        rows,
        vec![
            (101, 0, "pending".to_string(), legacy.clone(), false),
            (102, 0, "retrying".to_string(), legacy.clone(), false),
            (103, 0, "delivered".to_string(), legacy.clone(), false),
            (104, 0, "failed".to_string(), legacy, true),
        ],
        "records, event IDs and payloads survive; legacy rows belong to the environment endpoint"
    );
    let attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM registration_gate_webhook_attempts WHERE endpoint_id = 0",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts, 4);
    let baseline: i64 =
        sqlx::query_scalar("SELECT baseline_event_id FROM registration_gate_webhook_state")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(baseline, 41);

    pool.close().await;
    admin
        .execute(format!("DROP DATABASE {database} WITH (FORCE)").as_str())
        .await
        .unwrap();
}
