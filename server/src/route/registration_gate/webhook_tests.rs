use super::test_support::{
    observation, request, GateTestApp, TestUser, DOWN, GATE_ROUTE_TEST_LOCK, HEALTHY, OFF,
};
use application::service::registration_gate_webhook::DeliverRegistrationGateWebhooksUseCase;
use axum::http::{Method, StatusCode};
use driver::webhook::{HostResolver, HttpWebhookSender};
use kernel::prelude::entity::WebhookDeliveryPolicy;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;
use wiremock::MockServer;

const GATE: &str = "/api/v1/admin/registration-gate";
const WEBHOOK: &str = "/api/v1/admin/registration-gate/webhook";
const SECRET: &str = "whsec_route_test_secret_0123456789";
const URL: &str = "https://hooks.example.com:8443/very/secret/path?token=urlsecret";

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

fn blocked_sender() -> HttpWebhookSender {
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

fn endpoints() -> Vec<(Method, String, Option<serde_json::Value>)> {
    vec![
        (Method::GET, WEBHOOK.to_string(), None),
        (Method::POST, format!("{WEBHOOK}/test"), None),
        (
            Method::POST,
            format!("{WEBHOOK}/retry"),
            Some(serde_json::json!({})),
        ),
    ]
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn webhook_endpoints_reject_unauthenticated_and_non_admin_callers() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = GateTestApp::with_webhook_sender(&keto, blocked_sender()).await;

    for (method, uri, body) in endpoints() {
        let (status, _, _) = call(&app, None, method.clone(), &uri, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        let (status, _, text) = call(&app, Some(&app.outsider), method.clone(), &uri, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
        assert!(!text.contains(SECRET));
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unset_webhook_is_reported_as_not_configured_and_is_not_an_error() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = GateTestApp::new(&keto).await;

    let (status, body, _) = call(&app, Some(&app.admin_a), Method::GET, WEBHOOK, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["configuration"], "not_configured");
    assert!(body["destination_host"].is_null());
    assert!(body["issue"].is_null());
    assert_eq!(body["counts"]["failed"], 0);
    assert_eq!(body["recent_deliveries"], serde_json::json!([]));

    let (status, _, text) = call(
        &app,
        Some(&app.admin_a),
        Method::POST,
        &format!("{WEBHOOK}/test"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(text.contains("not configured"));

    let (status, body, _) = call(
        &app,
        Some(&app.admin_a),
        Method::POST,
        &format!("{WEBHOOK}/retry"),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["requeued"], 0);

    // 配送 worker 相当のサイクルは何もせず、ゲートの動作に影響しない
    let report = run_cycle(&app).await;
    assert!(report.skipped);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn misconfigured_webhook_is_reported_without_values_and_cannot_send_tests() {
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
        let (status, body, text) = call(&app, Some(&app.admin_a), Method::GET, WEBHOOK, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["configuration"], "misconfigured");
        assert_eq!(body["issue"], issue);
        assert!(body["destination_host"].is_null());
        assert!(!text.contains(SECRET) && !text.contains("10.0.0.1"));
        let (status, _, _) = call(
            &app,
            Some(&app.admin_a),
            Method::POST,
            &format!("{WEBHOOK}/test"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
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
async fn admin_can_inspect_test_and_retry_while_the_gate_and_notifications_stay_untouched() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = GateTestApp::with_webhook_sender(&keto, blocked_sender()).await;
    let admin = &app.admin_a;

    // 有効化を観測させてから (baseline)、通知イベントを発生させる
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

    // 名前が内部アドレスに解決される送信先は SSRF として恒久失敗 (再試行しない) になる
    let report = run_cycle(&app).await;
    assert_eq!(report.failed as usize, event_count);

    let (status, body, text) = call(&app, Some(admin), Method::GET, WEBHOOK, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["configuration"], "configured");
    assert_eq!(body["destination_host"], "hooks.example.com");
    assert_eq!(body["counts"]["failed"], event_count);
    let first = &body["recent_deliveries"][0];
    assert_eq!(first["status"], "failed");
    assert_eq!(first["last_result"], "ssrf_blocked");
    assert_eq!(first["failure_reason"], "permanent_error");
    assert_eq!(first["attempt_count"], 1);
    // Match the port as part of a host:port/URL, not inside the numeric snowflake event ID.
    for leaked in [SECRET, "urlsecret", "very/secret", ":8443", "https://"] {
        assert!(!text.contains(leaked), "status leaks {leaked}: {text}");
    }

    // テスト送信は実配送と同じ経路 (宛先検査) を通り、結果を返すだけ
    let (status, body, text) = call(
        &app,
        Some(admin),
        Method::POST,
        &format!("{WEBHOOK}/test"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["delivered"], false);
    assert_eq!(body["result"], "ssrf_blocked");
    assert!(body["event_id"].as_str().unwrap().starts_with("test_"));
    assert!(!text.contains(SECRET));

    // 再試行 API
    let (status, _, _) = call(
        &app,
        Some(admin),
        Method::POST,
        &format!("{WEBHOOK}/retry"),
        Some(serde_json::json!({"event_id": "not-a-number"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = call(
        &app,
        Some(admin),
        Method::POST,
        &format!("{WEBHOOK}/retry"),
        Some(serde_json::json!({"event_id": "12345"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body, _) = call(
        &app,
        Some(admin),
        Method::POST,
        &format!("{WEBHOOK}/retry"),
        Some(serde_json::json!({"event_id": event_id})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["requeued"], 1);
    let (_, body, _) = call(&app, Some(admin), Method::GET, WEBHOOK, None).await;
    assert_eq!(body["counts"]["pending"], 1);
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

    // ゲート状態・通知一覧・未読件数は Webhook の失敗・テスト送信・再試行で変わらない
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
fn openapi_documents_the_webhook_contract_and_delivery_guarantees() {
    let spec: serde_json::Value =
        serde_json::from_str(&crate::openapi::generate_openapi_json()).expect("valid OpenAPI");
    for (method, path, statuses) in [
        ("get", "", vec!["200", "401", "403"]),
        ("post", "/test", vec!["200", "401", "403", "409"]),
        ("post", "/retry", vec!["200", "400", "401", "403", "404"]),
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
        "unset",
        "disabled",
        "not_configured",
        "never changes the gate state",
        "HMAC-SHA256",
    ] {
        assert!(
            description.contains(needle),
            "tag description mentions {needle}"
        );
    }
    let status_description = spec["paths"][WEBHOOK]["get"]["description"]
        .as_str()
        .unwrap();
    assert!(status_description.contains("not_configured"));
    assert!(status_description.contains("at-least-once"));
    for schema in [
        "WebhookStatusResponse",
        "WebhookTestResponse",
        "WebhookRetryRequest",
        "WebhookRetryResponse",
        "WebhookDeliveryResponse",
    ] {
        assert!(
            spec["components"]["schemas"][schema].is_object(),
            "{schema}"
        );
    }
    let serialized =
        serde_json::to_string(&spec["components"]["schemas"]["WebhookStatusResponse"]).unwrap();
    assert!(!serialized.contains("secret\":"));
}
