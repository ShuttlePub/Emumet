//! tracing に secret・登録者の個人情報が出ないことの回帰テスト (実 DB)。
//!
//! production と同じ fmt 出力形式で TRACE まで捕捉し (`LogCapture`)、Webhook の管理・配送・環境変数の
//! 起動ログ・登録 guard の hook の各経路を失敗パスを含めて通したあと、secret / URL のトークン /
//! 暗号鍵 / メールアドレス / 確認コード相当が出力に無いことを確認する。

use super::shutdown_tests::generate_gate_events;
use super::test_support::{GateTestApp, GATE_ROUTE_TEST_LOCK};
use super::webhook_tests::{
    blocked_env_sender, call, key, loopback_sender, InternalResolver, ENDPOINTS, WEBHOOK,
};
use crate::registration_guard::{router, GuardApi, GuardConfig};
use crate::webhook_worker;
use application::service::registration_gate_webhook::{
    DeliverRegistrationGateWebhooksUseCase, ImportEnvironmentWebhookEndpointUseCase,
};
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use driver::webhook::{AeadWebhookSecretCipher, HttpWebhookSender};
use kernel::interfaces::repository::EnforcementProbeId;
use kernel::prelude::entity::WebhookDeliveryPolicy;
use kernel::test_utils::LogCapture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY_BYTE: u8 = 5;

/// 暗号鍵 (全バイト 5) が取りうる表記: hex・base64・Debug の配列。
fn key_renderings() -> Vec<(&'static str, String)> {
    vec![
        ("encryption key (hex)", "05".repeat(32)),
        (
            "encryption key (base64)",
            format!("{}BQU=", "BQUF".repeat(10)),
        ),
        (
            "encryption key (debug bytes)",
            vec![KEY_BYTE.to_string(); 8].join(", "),
        ),
    ]
}

async fn post_json(
    app: &GateTestApp,
    user: &super::test_support::TestUser,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, String) {
    let (status, _, text) = call(app, Some(user), Method::POST, uri, Some(body)).await;
    (status, text)
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn webhook_management_and_delivery_never_log_secrets_urls_or_keys() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let capture = LogCapture::start();
    let keto = MockServer::start().await;
    let healthy = MockServer::start().await;
    let failing = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(204))
        .mount(&healthy)
        .await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(500))
        .mount(&failing)
        .await;

    let secret_one = "whsec_leakcheck_secret_one_0123456789";
    let secret_two = "whsec_leakcheck_secret_two_9876543210";
    let secret_three = "whsec_leakcheck_secret_three_abcdefgh";
    let secret_bad = "whsec_leakcheck_rejected_create_FFFF6666";
    let secret_conflict = "whsec_leakcheck_conflicting_update_GGGG7777";
    let path_a = "/leak/path-TokenAAAA1111";
    let query_a = "QueryTokBBBB2222";
    let path_b = "/leak/path-TokenCCCC3333";
    let query_b = "QueryTokDDDD4444";
    let path_new = "/leak/path-TokenHHHH8888";
    let userinfo_password = "UserInfoPwEEEE5555";
    let http_token = "HttpTokIIII9999";

    let app = GateTestApp::with_webhook_wiring(&keto, loopback_sender(), key()).await;
    let admin = &app.admin_a;

    let (status, a, text) = call(
        &app,
        Some(admin),
        Method::POST,
        ENDPOINTS,
        Some(serde_json::json!({
            "url": format!("{}{path_a}?token={query_a}", healthy.uri()),
            "secret": secret_one, "categories": ["outage", "recovery"]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let (status, b, text) = call(
        &app,
        Some(admin),
        Method::POST,
        ENDPOINTS,
        Some(serde_json::json!({
            "url": format!("{}{path_b}?token={query_b}", failing.uri()),
            "secret": secret_two, "categories": ["outage", "recovery", "auto_stop", "override"]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let (id_a, id_b) = (a["id"].as_str().unwrap(), b["id"].as_str().unwrap());

    // 失敗パス: userinfo 付き・http・重複・revision 競合・存在しない ID・非管理者・未認証。
    for (body, expected) in [
        (
            serde_json::json!({
                "url": format!("https://user:{userinfo_password}@hooks.example.net/x"),
                "secret": secret_bad, "categories": ["outage"]
            }),
            StatusCode::BAD_REQUEST,
        ),
        (
            serde_json::json!({
                "url": format!("http://hooks.example.net/{http_token}"),
                "secret": secret_bad, "categories": ["outage"]
            }),
            StatusCode::BAD_REQUEST,
        ),
        (
            serde_json::json!({
                "url": format!("{}{path_a}?token={query_a}", healthy.uri()),
                "secret": secret_bad, "categories": ["outage"]
            }),
            StatusCode::CONFLICT,
        ),
    ] {
        let (status, text) = post_json(&app, admin, ENDPOINTS, body.clone()).await;
        assert_eq!(status, expected, "{body} -> {text}");
    }
    let (status, _, _) = call(
        &app,
        Some(admin),
        Method::PUT,
        &format!("{ENDPOINTS}/{id_a}"),
        Some(serde_json::json!({
            "revision": 99, "url": format!("{}{path_new}", healthy.uri()),
            "secret": secret_conflict
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _, _) = call(
        &app,
        Some(admin),
        Method::GET,
        &format!("{ENDPOINTS}/424242"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = call(
        &app,
        Some(&app.outsider),
        Method::PUT,
        &format!("{ENDPOINTS}/{id_a}"),
        Some(serde_json::json!({"revision": 1, "secret": secret_conflict})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(&app, None, Method::GET, ENDPOINTS, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 更新 (secret と URL の置き換え)。
    let (status, _, text) = call(
        &app,
        Some(admin),
        Method::PUT,
        &format!("{ENDPOINTS}/{id_a}"),
        Some(serde_json::json!({
            "revision": 1,
            "url": format!("{}{path_new}", healthy.uri()),
            "secret": secret_three,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");

    // 配送 (成功・5xx の再試行)・テスト送信・再試行。
    generate_gate_events(&app, 2).await;
    super::webhook_tests::run_cycle(&app).await;
    super::webhook_tests::run_cycle(&app).await;
    for id in [id_a, id_b] {
        let (status, _, _) = call(
            &app,
            Some(admin),
            Method::POST,
            &format!("{ENDPOINTS}/{id}/test"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _) = post_json(
        &app,
        admin,
        &format!("{WEBHOOK}/retry"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(
        &app,
        Some(admin),
        Method::GET,
        &format!("{WEBHOOK}/failures"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 復号できない secret (別の鍵で起動した worker) と、接続時の SSRF 拒否 (厳格な宛先検査)。
    generate_gate_events(&app, 1).await;
    let jitter = || 1.0;
    let stop = || false;
    let other_key = app
        .module
        .clone()
        .with_webhook_secret_cipher(AeadWebhookSecretCipher::from_key_bytes([9u8; 32]));
    other_key
        .deliver_registration_gate_webhooks(
            "leak-check-wrong-key",
            &WebhookDeliveryPolicy::default(),
            &jitter,
            &stop,
        )
        .await
        .expect("delivery cycle");
    let strict = app.module.clone().with_webhook_sender(blocked_env_sender());
    strict
        .deliver_registration_gate_webhooks(
            "leak-check-strict",
            &WebhookDeliveryPolicy::default(),
            &jitter,
            &stop,
        )
        .await
        .expect("delivery cycle");

    // 削除 (revision 競合と成功)。
    let (status, _) = post_json(
        &app,
        admin,
        &format!("{ENDPOINTS}/{id_b}/delete"),
        serde_json::json!({"revision": 99}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = post_json(
        &app,
        admin,
        &format!("{ENDPOINTS}/{id_b}/delete"),
        serde_json::json!({"revision": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 捕捉できていること (SQL の実行ログと、Report を表示するエラー経路のログが見える) を確認してから検査する。
    capture.assert_sees("sqlx::query");
    capture.assert_sees("Concurrency conflict");
    let mut forbidden: Vec<(&str, &str)> = vec![
        ("secret one", secret_one),
        ("secret two", secret_two),
        ("secret three", secret_three),
        ("rejected create secret", secret_bad),
        ("conflicting update secret", secret_conflict),
        ("url path a", path_a),
        ("url query a", query_a),
        ("url path b", path_b),
        ("url query b", query_b),
        ("replacement url path", path_new),
        ("userinfo password", userinfo_password),
        ("http url token", http_token),
        ("any url path", "/leak/"),
    ];
    let keys = key_renderings();
    forbidden.extend(keys.iter().map(|(label, value)| (*label, value.as_str())));
    capture.assert_absent(&forbidden);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn webhook_environment_destination_startup_and_worker_logs_name_only_the_host() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let capture = LogCapture::start();
    let keto = MockServer::start().await;
    let secret = "whsec_env_leakcheck_secret_0123456789";
    let url_path = "/env-leak/path-TokenJJJJ0000";
    let url_query = "QueryTokKKKK1111";
    let url = format!("https://hooks.example.com:8443{url_path}?token={url_query}");

    let sender = HttpWebhookSender::from_values(Some(&url), Some(secret), None)
        .with_resolver(Arc::new(InternalResolver));
    let app = GateTestApp::with_webhook_wiring(&keto, sender, key()).await;
    generate_gate_events(&app, 2).await;

    // Debug 表現にも secret・URL・鍵は出ない。
    let debug = format!(
        "{:?} {:?}",
        HttpWebhookSender::from_values(Some(&url), Some(secret), None),
        AeadWebhookSecretCipher::from_key_bytes([KEY_BYTE; 32])
    );
    for leaked in [secret, url_path, url_query] {
        assert!(!debug.contains(leaked), "Debug output leaks {leaked}");
    }

    // 起動と同じ手順: baseline (取り込み)・worker の起動・配送 (宛先検査で恒久失敗)・停止。
    let module = Arc::new(app.module.clone());
    webhook_worker::prepare_baseline(&module).await.unwrap();
    let worker = webhook_worker::spawn(module.clone());
    for _ in 0..200 {
        let (_, list, _) = call(&app, Some(&app.admin_a), Method::GET, ENDPOINTS, None).await;
        if list["has_failed_deliveries"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(worker.stop_and_wait(Duration::from_secs(10)).await);

    // 設定不備の環境変数 (http) の起動ログ。
    let misconfigured = app
        .module
        .clone()
        .with_webhook_sender(HttpWebhookSender::from_values(
            Some(&format!(
                "http://hooks.example.com{url_path}?token={url_query}"
            )),
            Some(secret),
            None,
        ));
    let _ = misconfigured.import_environment_webhook_endpoint().await;
    let worker = webhook_worker::spawn(Arc::new(misconfigured));
    assert!(worker.stop_and_wait(Duration::from_secs(10)).await);

    capture.assert_sees("registration gate webhook");
    let mut forbidden: Vec<(&str, &str)> = vec![
        ("environment secret", secret),
        ("environment url path", url_path),
        ("environment url query", url_query),
    ];
    let keys = key_renderings();
    forbidden.extend(keys.iter().map(|(label, value)| (*label, value.as_str())));
    capture.assert_absent(&forbidden);
}

const GUARD_SECRET: &str = "leakcheck-guard-shared-secret-0123456789abcdef";

fn hook_request(secret: Option<&str>, peer: &str, body: String) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/registration/pre-persist")
        .extension(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    if let Some(secret) = secret {
        request = request.header("authorization", format!("Bearer {secret}"));
    }
    request.body(Body::from(body)).unwrap()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn registration_guard_hook_never_logs_emails_proofs_tokens_or_the_shared_secret() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let capture = LogCapture::start();
    let keto = MockServer::start().await;
    let app = GateTestApp::new(&keto).await;
    let config = GuardConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        allowed_peers: vec!["127.0.0.1".parse().unwrap()],
        secret: GUARD_SECRET.to_string(),
        deadline: Duration::from_secs(5),
        period: Duration::from_secs(30),
    };
    let hook = router(config, GuardApi(Arc::new(app.module.clone())));

    let email = "leak.registrant+case@example.com";
    let proof = "EmailProofLeakCheckLLLL2222";
    let token = "BotTokenLeakCheckMMMM3333";
    let password = "PasswordLeakCheckNNNN4444";
    let flow = "f68e7ed2-b053-430d-acba-22883c94c998";
    let probe_id = EnforcementProbeId::parse(&"ab".repeat(16)).unwrap();
    let canary_email = probe_id.email();
    let valid = |email: &str| {
        serde_json::json!({
            "flow_id": flow, "flow_type": "api", "email": email,
            "email_proof": proof, "bot_token": token,
        })
    };
    let mut with_password = valid(email);
    with_password["credentials"] = serde_json::json!({"password": password});
    let mut bad_flow = valid(email);
    bad_flow["flow_id"] = "not-a-flow".into();
    let mut canary = valid(&canary_email);
    canary["registration_probe_id"] = probe_id.as_str().into();

    for (secret, peer, body) in [
        (Some(GUARD_SECRET), "127.0.0.1:1", valid(email).to_string()),
        (
            Some(GUARD_SECRET),
            "127.0.0.1:1",
            valid(&email.to_uppercase()).to_string(),
        ),
        (Some(GUARD_SECRET), "127.0.0.1:1", canary.to_string()),
        (Some(GUARD_SECRET), "127.0.0.1:1", with_password.to_string()),
        (Some(GUARD_SECRET), "127.0.0.1:1", bad_flow.to_string()),
        (
            Some(GUARD_SECRET),
            "127.0.0.1:1",
            format!("{{\"email\":\"{email}\""),
        ),
        (
            Some("wrong-secret-OOOO5555-0123456789abcdef"),
            "127.0.0.1:1",
            valid(email).to_string(),
        ),
        (Some(GUARD_SECRET), "192.0.2.10:1", valid(email).to_string()),
        (None, "127.0.0.1:1", valid(email).to_string()),
    ] {
        let response = hook
            .clone()
            .oneshot(hook_request(secret, peer, body))
            .await
            .unwrap();
        assert!(
            matches!(
                response.status(),
                StatusCode::NO_CONTENT | StatusCode::FORBIDDEN
            ),
            "{}",
            response.status()
        );
    }

    capture.assert_sees("sqlx::query");
    capture.assert_absent(&[
        ("guard shared secret", GUARD_SECRET),
        ("wrong shared secret", "wrong-secret-OOOO5555"),
        ("registrant email", email),
        ("registrant local part", "leak.registrant"),
        ("canary email", canary_email.as_str()),
        ("canary id", probe_id.as_str()),
        ("email proof", proof),
        ("bot token", token),
        ("submitted password", password),
    ]);
}
