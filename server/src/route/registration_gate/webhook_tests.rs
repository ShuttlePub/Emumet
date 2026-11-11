use super::test_support::{
    observation, request, GateTestApp, TestUser, DOWN, GATE_ROUTE_TEST_LOCK, HEALTHY, OFF,
};
use application::service::registration_gate_webhook::DeliverRegistrationGateWebhooksUseCase;
use axum::http::{Method, StatusCode};
use driver::webhook::{AeadWebhookSecretCipher, HostResolver, HttpWebhookSender};
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::prelude::entity::WebhookDeliveryPolicy;
use sqlx::PgConnection;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

const GATE: &str = "/api/v1/admin/registration-gate";
const WEBHOOK: &str = "/api/v1/admin/registration-gate/webhook";
const ENDPOINTS: &str = "/api/v1/admin/registration-gate/webhook/endpoints";
const SECRET: &str = "whsec_route_test_secret_0123456789";
const SECRET_2: &str = "whsec_route_second_secret_9876543210";
const URL_SECRET_PATH: &str = "/very/secret/path/abcdefghij-a8f2";
const URL: &str = "https://hooks.example.com:8443/very/secret/path/abcdefghij-a8f2?token=QueryTokZk9";

struct InternalResolver;

impl HostResolver for InternalResolver {
    fn resolve<'a>(
        &'a self,
        _host: &'a str,
        port: u16,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + 'a>,
    > {
        Box::pin(async move { Ok(vec![SocketAddr::new("127.0.0.1".parse().unwrap(), port)]) })
    }
}

fn key() -> AeadWebhookSecretCipher {
    AeadWebhookSecretCipher::from_key_bytes([5u8; 32])
}

/// 環境変数の送信先なし + loopback を許す test 用の宛先検査。
fn loopback_sender() -> HttpWebhookSender {
    HttpWebhookSender::disabled_allowing_insecure_loopback_destinations()
}

/// 名前が内部アドレスに解決される (本番と同じ厳格な宛先検査の) sender。
fn blocked_env_sender() -> HttpWebhookSender {
    HttpWebhookSender::from_values(Some(URL), Some(SECRET), None)
        .with_resolver(Arc::new(InternalResolver))
}

async fn call(
    app: &GateTestApp,
    user: Option<&TestUser>,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value, String) {
    let response = app
        .router
        .clone()
        .oneshot(request(user, method, uri, body))
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json, text)
}

fn all_routes() -> Vec<(Method, String, Option<serde_json::Value>)> {
    let body = |value: serde_json::Value| Some(value);
    vec![
        (Method::GET, ENDPOINTS.to_string(), None),
        (
            Method::POST,
            ENDPOINTS.to_string(),
            body(serde_json::json!({
                "url": "https://hooks.example.net/h", "secret": SECRET, "categories": ["outage"]
            })),
        ),
        (Method::GET, format!("{ENDPOINTS}/1"), None),
        (
            Method::PUT,
            format!("{ENDPOINTS}/1"),
            body(serde_json::json!({"revision": 1, "categories": ["outage"]})),
        ),
        (
            Method::POST,
            format!("{ENDPOINTS}/1/delete"),
            body(serde_json::json!({"revision": 1})),
        ),
        (Method::POST, format!("{ENDPOINTS}/1/test"), None),
        (
            Method::POST,
            format!("{WEBHOOK}/retry"),
            body(serde_json::json!({})),
        ),
        (Method::GET, format!("{WEBHOOK}/failures"), None),
    ]
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn every_webhook_route_rejects_unauthenticated_and_non_admin_callers() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = GateTestApp::with_webhook_wiring(&keto, loopback_sender(), key()).await;

    for (method, uri, body) in all_routes() {
        let (status, _, _) = call(&app, None, method.clone(), &uri, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        let (status, _, text) = call(&app, Some(&app.outsider), method.clone(), &uri, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
        assert!(!text.contains(SECRET));
    }
    // 拒否された呼び出しは何も作らない。
    let (_, list, _) = call(&app, Some(&app.admin_a), Method::GET, ENDPOINTS, None).await;
    assert_eq!(list["endpoints"], serde_json::json!([]));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn nothing_configured_lists_no_endpoints_and_is_not_an_error() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = GateTestApp::new(&keto).await;

    let (status, body, _) = call(&app, Some(&app.admin_a), Method::GET, ENDPOINTS, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["endpoints"], serde_json::json!([]));
    assert_eq!(body["has_failed_deliveries"], false);
    assert_eq!(body["encryption_key_configured"], false);
    assert_eq!(body["max_endpoints"], 10);

    let (status, _, text) = call(
        &app,
        Some(&app.admin_a),
        Method::POST,
        &format!("{ENDPOINTS}/environment/test"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(text.contains("not configured"));
    // 鍵がないと送信先は作れない (secret を平文で保存しない)。
    let (status, _, text) = call(
        &app,
        Some(&app.admin_a),
        Method::POST,
        ENDPOINTS,
        Some(serde_json::json!({
            "url": "https://hooks.example.net/h", "secret": SECRET, "categories": ["outage"]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(text, "encryption_key_not_configured");
    for bad_id in ["abc", "0", "-5"] {
        let (status, _, _) = call(
            &app,
            Some(&app.admin_a),
            Method::GET,
            &format!("{ENDPOINTS}/{bad_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad_id}");
    }
    let (status, _, _) = call(
        &app,
        Some(&app.admin_a),
        Method::GET,
        &format!("{ENDPOINTS}/123456"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // 配送 worker 相当のサイクルは何もせず、ゲートの動作に影響しない。
    let report = run_cycle(&app).await;
    assert!(report.skipped);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn misconfigured_environment_destination_is_listed_read_only_without_values() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    for (sender, issue) in [
        (
            HttpWebhookSender::from_values(Some("https://10.0.0.1/hook"), Some(SECRET), None),
            "forbidden_destination",
        ),
        (
            HttpWebhookSender::from_values(Some("http://hooks.example.com/"), Some(SECRET), None),
            "unsupported_scheme",
        ),
        (
            HttpWebhookSender::from_values(Some("https://hooks.example.com/"), None, None),
            "missing_secret",
        ),
    ] {
        let app = GateTestApp::with_webhook_sender(&keto, sender).await;
        let (status, body, text) =
            call(&app, Some(&app.admin_a), Method::GET, ENDPOINTS, None).await;
        assert_eq!(status, StatusCode::OK);
        let env = &body["endpoints"][0];
        assert_eq!(env["id"], "environment");
        assert_eq!(env["source"], "environment");
        assert_eq!(env["editable"], false);
        assert_eq!(env["status"], "failing");
        assert_eq!(env["config_issue"], issue);
        assert!(env["revision"].is_null());
        assert!(!text.contains(SECRET) && !text.contains("10.0.0.1"));
        let (status, _, _) = call(
            &app,
            Some(&app.admin_a),
            Method::POST,
            &format!("{ENDPOINTS}/environment/test"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _, text) = call(
            &app,
            Some(&app.admin_a),
            Method::PUT,
            &format!("{ENDPOINTS}/environment"),
            Some(serde_json::json!({"revision": 1, "categories": ["outage"]})),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(text, "environment_endpoint_read_only");
    }
}

async fn run_cycle(app: &GateTestApp) -> kernel::interfaces::repository::WebhookCycleReport {
    let jitter = || 1.0;
    let stop = || false;
    app.module
        .deliver_registration_gate_webhooks(
            "route-test-worker",
            &WebhookDeliveryPolicy::default(),
            &jitter,
            &stop,
        )
        .await
        .expect("delivery cycle")
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn admin_manages_endpoints_over_http_without_secret_or_url_ever_leaking() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let receiver = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(204))
        .mount(&receiver)
        .await;
    let app = GateTestApp::with_webhook_wiring(&keto, loopback_sender(), key()).await;
    let admin = &app.admin_a;
    let hook_url = format!("{}{URL_SECRET_PATH}?token=QueryTokZk9", receiver.uri());
    let mut transcript = String::new();

    // 作成: 201 とマスク形の URL だけ。全 URL・path・query・secret は返らない。
    let (status, created, text) = call(
        &app,
        Some(admin),
        Method::POST,
        ENDPOINTS,
        Some(serde_json::json!({
            "url": hook_url, "secret": SECRET, "categories": ["outage", "auto_stop"]
        })),
    )
    .await;
    transcript.push_str(&text);
    assert_eq!(status, StatusCode::CREATED);
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["source"], "api");
    assert_eq!(created["revision"], 1);
    assert_eq!(created["status"], "healthy");
    assert_eq!(
        created["categories"],
        serde_json::json!(["outage", "auto_stop"])
    );
    assert_eq!(created["secret_configured"], true);
    assert_eq!(created["editable"], true);
    assert_eq!(
        created["masked_url"],
        format!("{}/••••/a8f2", receiver.address())
    );
    assert!(created.get("url").is_none() && created.get("secret").is_none());

    // 重複 URL (別表記) は 409、不正な入力は 400 (固定コード)。
    let (status, _, text) = call(
        &app,
        Some(admin),
        Method::POST,
        ENDPOINTS,
        Some(serde_json::json!({
            "url": hook_url, "secret": SECRET_2, "categories": ["recovery"]
        })),
    )
    .await;
    transcript.push_str(&text);
    assert_eq!(
        (status, text.as_str()),
        (StatusCode::CONFLICT, "duplicate_url")
    );
    for (body, code) in [
        (
            serde_json::json!({"url": "https://hooks.example.net/h", "secret": "short", "categories": ["outage"]}),
            "secret_too_short",
        ),
        (
            serde_json::json!({"url": "https://hooks.example.net/h", "secret": SECRET, "categories": []}),
            "no_categories",
        ),
    ] {
        let (status, _, text) = call(&app, Some(admin), Method::POST, ENDPOINTS, Some(body)).await;
        transcript.push_str(&text);
        assert_eq!((status, text.as_str()), (StatusCode::BAD_REQUEST, code));
    }
    // 未知のカテゴリ (webhook_test を含む) は選択できない。本文は echo されない。
    let (status, _, text) = call(
        &app,
        Some(admin),
        Method::POST,
        ENDPOINTS,
        Some(serde_json::json!({
            "url": "https://hooks.example.net/h", "secret": SECRET, "categories": ["webhook_test"]
        })),
    )
    .await;
    transcript.push_str(&text);
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!text.contains("webhook_test") && !text.contains(SECRET));

    // 一覧・取得。
    let (status, list, text) = call(&app, Some(admin), Method::GET, ENDPOINTS, None).await;
    transcript.push_str(&text);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["endpoints"].as_array().unwrap().len(), 1);
    assert_eq!(list["encryption_key_configured"], true);
    let (status, fetched, text) = call(
        &app,
        Some(admin),
        Method::GET,
        &format!("{ENDPOINTS}/{id}"),
        None,
    )
    .await;
    transcript.push_str(&text);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched, created);

    // 更新: revision 不一致は 409。省略した URL/secret は変わらない。
    let (status, _, _) = call(
        &app,
        Some(admin),
        Method::PUT,
        &format!("{ENDPOINTS}/{id}"),
        Some(serde_json::json!({"revision": 9, "categories": ["recovery"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, updated, text) = call(
        &app,
        Some(admin),
        Method::PUT,
        &format!("{ENDPOINTS}/{id}"),
        Some(serde_json::json!({"revision": 1, "categories": ["recovery", "override"]})),
    )
    .await;
    transcript.push_str(&text);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["revision"], 2);
    assert_eq!(updated["masked_url"], created["masked_url"]);
    assert_eq!(
        updated["categories"],
        serde_json::json!(["recovery", "override"])
    );
    let (status, _, text) = call(
        &app,
        Some(admin),
        Method::PUT,
        &format!("{ENDPOINTS}/{id}"),
        Some(serde_json::json!({"revision": 2})),
    )
    .await;
    assert_eq!(
        (status, text.as_str()),
        (StatusCode::BAD_REQUEST, "nothing_to_update")
    );

    // テスト送信: その送信先の secret で署名して 1 回だけ送る。
    let (status, tested, text) = call(
        &app,
        Some(admin),
        Method::POST,
        &format!("{ENDPOINTS}/{id}/test"),
        None,
    )
    .await;
    transcript.push_str(&text);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tested["delivered"], true);
    assert_eq!(tested["result"], "delivered");
    assert!(tested["event_id"].as_str().unwrap().starts_with("test_"));
    assert_eq!(receiver.received_requests().await.unwrap().len(), 1);

    // 削除は revision 一致が必要。
    let (status, _, _) = call(
        &app,
        Some(admin),
        Method::POST,
        &format!("{ENDPOINTS}/{id}/delete"),
        Some(serde_json::json!({"revision": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _, _) = call(
        &app,
        Some(admin),
        Method::POST,
        &format!("{ENDPOINTS}/{id}/delete"),
        Some(serde_json::json!({"revision": 2})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = call(
        &app,
        Some(admin),
        Method::GET,
        &format!("{ENDPOINTS}/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    for leaked in [
        SECRET,
        SECRET_2,
        "QueryTokZk9",
        "very/secret",
        "abcdefghij",
        "token=",
    ] {
        assert!(
            !transcript.contains(leaked),
            "responses leak {leaked}: {transcript}"
        );
    }
    // 監査 (DB) にも URL・secret は残らない。
    let mut db = app.module.database_connection().connection().await.unwrap();
    let con: &mut PgConnection = &mut db;
    let audit: String = sqlx::query_scalar(
        "SELECT COALESCE(string_agg(row_to_json(a)::text, ' '), '') \
         FROM registration_gate_webhook_endpoint_audit a",
    )
    .fetch_one(con)
    .await
    .unwrap();
    for leaked in [SECRET, "QueryTokZk9", "very/secret", "127.0.0.1"] {
        assert!(!audit.contains(leaked), "audit leaks {leaked}");
    }
    assert!(
        audit.contains("\"operation\":\"create\"") && audit.contains("\"operation\":\"delete\"")
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn failed_deliveries_are_paged_over_http_past_fifty_and_retried_one_by_one() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let receiver = MockServer::start().await;
    let app = GateTestApp::with_webhook_wiring(&keto, loopback_sender(), key()).await;
    let admin = &app.admin_a;
    let (_, created, _) = call(
        &app,
        Some(admin),
        Method::POST,
        ENDPOINTS,
        Some(serde_json::json!({
            "url": format!("{}/hook", receiver.uri()), "secret": SECRET, "categories": ["outage"]
        })),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_string();
    let endpoint_id: i64 = id.parse().unwrap();
    {
        let mut db = app.module.database_connection().connection().await.unwrap();
        let con: &mut PgConnection = &mut db;
        let now = time::OffsetDateTime::now_utc();
        for index in 0..55i64 {
            sqlx::query(
                r#"
                INSERT INTO registration_gate_webhook_deliveries (
                    endpoint_id, event_id, kind, payload, status, attempt_count,
                    cycle_attempt_count, cycle_started_at, next_attempt_at, last_result,
                    last_http_status, failure_reason, failed_at, created_at, updated_at
                ) VALUES ($1, $2, 'protection_degraded', '{}', 'failed', 8, 8, $3, $3,
                          'http_server_error', 500, 'retries_exhausted', $4, $3, $3)
                "#,
            )
            .bind(endpoint_id)
            .bind(7000 + index)
            .bind(now - time::Duration::hours(1))
            .bind(now + time::Duration::seconds(index))
            .execute(&mut *con)
            .await
            .unwrap();
        }
    }
    let (_, list, _) = call(&app, Some(admin), Method::GET, ENDPOINTS, None).await;
    assert_eq!(list["has_failed_deliveries"], true);
    assert_eq!(list["endpoints"][0]["failed_count"], 55);
    assert_eq!(list["endpoints"][0]["status"], "failing");
    assert_eq!(
        list["endpoints"][0]["failing_reasons"],
        serde_json::json!(["failed_deliveries"])
    );

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let uri = match &cursor {
            Some(cursor) => format!("{WEBHOOK}/failures?endpoint_id={id}&limit=20&before={cursor}"),
            None => format!("{WEBHOOK}/failures?endpoint_id={id}&limit=20"),
        };
        let (status, page, text) = call(&app, Some(admin), Method::GET, &uri, None).await;
        assert_eq!(status, StatusCode::OK);
        for leaked in [SECRET, "127.0.0.1", "/hook"] {
            assert!(!text.contains(leaked), "failures leak {leaked}");
        }
        pages += 1;
        for item in page["items"].as_array().unwrap() {
            assert_eq!(item["endpoint_id"], id);
            assert_eq!(item["event_type"], "protection_degraded");
            assert_eq!(item["category"], "outage");
            assert_eq!(item["result"], "http_server_error");
            assert_eq!(item["failure_reason"], "retries_exhausted");
            seen.push(item["event_id"].as_str().unwrap().to_string());
        }
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen.len(), 55);
    assert_eq!(seen.first().unwrap(), "7054");
    assert_eq!(
        seen.last().unwrap(),
        "7000",
        "the oldest failure is reachable"
    );

    // 最古の failed を 1 件だけ再試行できる。
    let (status, body, _) = call(
        &app,
        Some(admin),
        Method::POST,
        &format!("{WEBHOOK}/retry"),
        Some(serde_json::json!({"endpoint_id": id, "event_id": "7000"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["requeued"], 1);
    let (status, _, _) = call(
        &app,
        Some(admin),
        Method::POST,
        &format!("{WEBHOOK}/retry"),
        Some(serde_json::json!({"endpoint_id": id, "event_id": "7000"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    for bad in [
        serde_json::json!({"event_id": "7001"}),
        serde_json::json!({"endpoint_id": id, "event_id": "x"}),
        serde_json::json!({"endpoint_id": "nope"}),
    ] {
        let (status, _, _) = call(
            &app,
            Some(admin),
            Method::POST,
            &format!("{WEBHOOK}/retry"),
            Some(bad),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (_, body, _) = call(
        &app,
        Some(admin),
        Method::GET,
        &format!("{WEBHOOK}/failures?endpoint_id={id}&limit=100"),
        None,
    )
    .await;
    assert_eq!(body["items"].as_array().unwrap().len(), 54);
    let (status, body, _) = call(
        &app,
        Some(admin),
        Method::POST,
        &format!("{WEBHOOK}/retry"),
        Some(serde_json::json!({"endpoint_id": id})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["requeued"], 54);
    let (_, list, _) = call(&app, Some(admin), Method::GET, ENDPOINTS, None).await;
    assert_eq!(list["has_failed_deliveries"], false);
    assert_eq!(list["endpoints"][0]["status"], "healthy");
    for bad_query in ["limit=abc", "before=not-a-cursor", "endpoint_id=abc"] {
        let (status, _, _) = call(
            &app,
            Some(admin),
            Method::GET,
            &format!("{WEBHOOK}/failures?{bad_query}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad_query}");
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn webhook_trouble_and_endpoint_management_never_touch_the_gate_or_notifications() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = GateTestApp::with_webhook_wiring(&keto, blocked_env_sender(), key()).await;
    let admin = &app.admin_a;

    run_cycle(&app).await;
    let (_, gate, _) = call(&app, Some(admin), Method::GET, GATE, None).await;
    let (status, _, _) = call(
        &app,
        Some(admin),
        Method::POST,
        &format!("{GATE}/override"),
        Some(serde_json::json!({
            "revision": gate["revision"],
            "acknowledge_warning": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    app.observe(observation(HEALTHY, OFF)).await;
    app.observe(observation(DOWN, OFF)).await;

    let (_, gate_before, _) = call(&app, Some(admin), Method::GET, GATE, None).await;
    let (_, notifications_before, _) = call(
        &app,
        Some(admin),
        Method::GET,
        &format!("{GATE}/notifications"),
        None,
    )
    .await;
    let event_id = notifications_before["notifications"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let event_count = notifications_before["notifications"]
        .as_array()
        .unwrap()
        .len();
    assert!(event_count >= 1);

    // 名前が内部アドレスに解決される環境変数の送信先は SSRF として恒久失敗 (再試行しない) になる。
    let report = run_cycle(&app).await;
    assert_eq!(report.failed as usize, event_count);

    let (status, body, text) = call(&app, Some(admin), Method::GET, ENDPOINTS, None).await;
    assert_eq!(status, StatusCode::OK);
    let env = &body["endpoints"][0];
    assert_eq!(env["id"], "environment");
    assert_eq!(env["status"], "failing");
    assert_eq!(env["failed_count"], event_count);
    assert_eq!(body["has_failed_deliveries"], true);
    for leaked in [SECRET, "QueryTokZk9", "very/secret", "https://", "abcdefghij"] {
        assert!(!text.contains(leaked), "list leaks {leaked}: {text}");
    }
    assert_eq!(env["masked_url"], "hooks.example.com:8443/••••/a8f2");

    let (_, failures, _) = call(
        &app,
        Some(admin),
        Method::GET,
        &format!("{WEBHOOK}/failures?endpoint_id=environment"),
        None,
    )
    .await;
    let first = &failures["items"][0];
    assert_eq!(first["endpoint_id"], "environment");
    assert_eq!(first["result"], "ssrf_blocked");
    assert_eq!(first["failure_reason"], "permanent_error");
    assert_eq!(first["attempt_count"], 1);

    // テスト送信 (環境変数の送信先) は実配送と同じ経路 (宛先検査) を通り、結果を返すだけ。
    let (status, body, text) = call(
        &app,
        Some(admin),
        Method::POST,
        &format!("{ENDPOINTS}/environment/test"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["delivered"], false);
    assert_eq!(body["result"], "ssrf_blocked");
    assert!(!text.contains(SECRET));

    // 再試行 (1 件 → 全体)。
    let (status, body, _) = call(
        &app,
        Some(admin),
        Method::POST,
        &format!("{WEBHOOK}/retry"),
        Some(serde_json::json!({"endpoint_id": "environment", "event_id": event_id})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["requeued"], 1);
    let (status, body, _) = call(
        &app,
        Some(admin),
        Method::POST,
        &format!("{WEBHOOK}/retry"),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["requeued"], event_count - 1);

    // 送信先の登録・更新・削除も含め、ゲート状態・通知一覧・未読件数は変わらない。
    let (status, created, _) = call(
        &app,
        Some(admin),
        Method::POST,
        ENDPOINTS,
        Some(serde_json::json!({
            "url": "https://hooks.example.net/other-token-0123456789", "secret": SECRET_2,
            "categories": ["override"]
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "this fixture resolves every host to an internal address, so registration is refused"
    );
    assert!(created.is_null() || created.is_string());
    let (_, gate_after, _) = call(&app, Some(admin), Method::GET, GATE, None).await;
    assert_eq!(gate_after, gate_before);
    let (_, notifications_after, _) = call(
        &app,
        Some(admin),
        Method::GET,
        &format!("{GATE}/notifications"),
        None,
    )
    .await;
    assert_eq!(notifications_after, notifications_before);
    let (_, unread, _) = call(
        &app,
        Some(&app.admin_b),
        Method::GET,
        &format!("{GATE}/notifications/unread-count"),
        None,
    )
    .await;
    assert_eq!(unread["unread_count"], event_count);
}

#[test]
fn openapi_documents_the_endpoint_management_contract_and_delivery_guarantees() {
    let spec: serde_json::Value =
        serde_json::from_str(&crate::openapi::generate_openapi_json()).expect("valid OpenAPI");
    for (method, path, statuses) in [
        ("get", "/endpoints", vec!["200", "401", "403"]),
        (
            "post",
            "/endpoints",
            vec!["201", "400", "401", "403", "409"],
        ),
        (
            "get",
            "/endpoints/{endpoint_id}",
            vec!["200", "400", "401", "403", "404"],
        ),
        (
            "put",
            "/endpoints/{endpoint_id}",
            vec!["200", "400", "401", "403", "404", "409"],
        ),
        (
            "post",
            "/endpoints/{endpoint_id}/delete",
            vec!["204", "401", "403", "404", "409"],
        ),
        (
            "post",
            "/endpoints/{endpoint_id}/test",
            vec!["200", "401", "403", "404", "409"],
        ),
        ("post", "/retry", vec!["200", "400", "401", "403", "404"]),
        ("get", "/failures", vec!["200", "400", "401", "403"]),
    ] {
        let operation = &spec["paths"][format!("{WEBHOOK}{path}")][method];
        assert!(operation.is_object(), "{method} {WEBHOOK}{path} registered");
        assert_eq!(
            operation["security"],
            serde_json::json!([{"bearer_auth": []}])
        );
        assert_eq!(
            operation["tags"],
            serde_json::json!(["RegistrationGateWebhook"])
        );
        for status in statuses {
            assert!(
                operation["responses"].get(status).is_some(),
                "{method} {WEBHOOK}{path} documents {status}"
            );
        }
    }
    for removed in ["", "/test"] {
        assert!(
            spec["paths"][format!("{WEBHOOK}{removed}")].is_null(),
            "the single-destination aggregate API {WEBHOOK}{removed} is replaced"
        );
    }
    let tag = spec["tags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "RegistrationGateWebhook")
        .expect("RegistrationGateWebhook tag");
    let description = tag["description"].as_str().unwrap();
    for needle in [
        "at-least-once",
        "de-duplicate",
        "not guaranteed",
        "never changes the gate state",
        "HMAC-SHA256",
        "REGISTRATION_WEBHOOK_ENCRYPTION_KEY",
        "cursor",
        "outage",
        "auto_stop",
    ] {
        assert!(
            description.contains(needle),
            "tag description mentions {needle}"
        );
    }
    let list_description = spec["paths"][ENDPOINTS]["get"]["description"]
        .as_str()
        .unwrap();
    assert!(list_description.contains("masked"));
    assert!(list_description.contains("at-least-once"));
    for schema in [
        "CreateWebhookEndpointRequest",
        "UpdateWebhookEndpointRequest",
        "DeleteWebhookEndpointRequest",
        "WebhookCategoryId",
        "WebhookEndpointResponse",
        "WebhookEndpointListResponse",
        "WebhookFailedDeliveryResponse",
        "WebhookFailedDeliveriesResponse",
        "WebhookTestResponse",
        "WebhookRetryRequest",
        "WebhookRetryResponse",
    ] {
        assert!(
            spec["components"]["schemas"][schema].is_object(),
            "{schema}"
        );
    }
    assert_eq!(
        spec["components"]["schemas"]["WebhookCategoryId"]["enum"],
        serde_json::json!(["outage", "recovery", "auto_stop", "override"])
    );
    for schema in [
        "WebhookEndpointResponse",
        "WebhookEndpointListResponse",
        "WebhookFailedDeliveryResponse",
    ] {
        let properties = &spec["components"]["schemas"][schema]["properties"];
        for forbidden in ["url", "secret", "path", "query", "payload"] {
            assert!(
                properties.get(forbidden).is_none(),
                "{schema} must not expose {forbidden}"
            );
        }
    }
    let create =
        &spec["components"]["schemas"]["CreateWebhookEndpointRequest"]["properties"]["secret"];
    assert_eq!(create["writeOnly"], true);
}
