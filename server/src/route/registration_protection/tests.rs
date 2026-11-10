//! 登録保護の HTTP 契約テスト (実 DB: DATABASE_URL)。provider が未設定の本番構成 (外部へ接続しない) で、
//! 認証・認可・入力検証・応答の秘匿・内部 use case を公開しないことを固定する。provider 経路の
//! 振る舞い (実検証・分類・proof) は driver / application の実 DB テストが担う。

use crate::route::registration_gate::test_support::{request, response_json, GateTestApp};
use axum::http::{Method, StatusCode};
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use tower::ServiceExt;
use wiremock::MockServer;

async fn app(keto: &MockServer) -> GateTestApp {
    let app = GateTestApp::new(keto).await;
    let mut executor = app
        .module
        .database_connection()
        .connection()
        .await
        .expect("database executor");
    sqlx::query(
        "TRUNCATE registration_protection_readiness, registration_email_challenges, \
         registration_email_send_log",
    )
    .execute(&mut *executor)
    .await
    .expect("reset registration protection");
    app
}

const LIST: &str = "/api/v1/admin/registration-gate/protections";
const ISSUE: &str = "/api/v1/registration/email-challenges";

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn admin_endpoints_require_authentication_and_the_admin_permission() {
    let _guard = crate::route::registration_gate::test_support::GATE_ROUTE_TEST_LOCK
        .lock()
        .await;
    let keto = MockServer::start().await;
    let app = app(&keto).await;

    for (method, uri, body) in [
        (Method::GET, LIST.to_string(), None),
        (
            Method::POST,
            format!("{LIST}/bot/verify"),
            Some(serde_json::json!({})),
        ),
        (
            Method::POST,
            format!("{LIST}/email/verify"),
            Some(serde_json::json!({})),
        ),
    ] {
        let response = app
            .router
            .clone()
            .oneshot(request(None, method.clone(), &uri, body.clone()))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{uri} without a token"
        );
        let response = app
            .router
            .clone()
            .oneshot(request(Some(&app.outsider), method, &uri, body))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{uri} for a non-admin"
        );
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unconfigured_providers_are_configuration_invalid_and_responses_carry_no_secrets() {
    let _guard = crate::route::registration_gate::test_support::GATE_ROUTE_TEST_LOCK
        .lock()
        .await;
    let keto = MockServer::start().await;
    let app = app(&keto).await;

    let response = app
        .router
        .clone()
        .oneshot(request(Some(&app.admin_a), Method::GET, LIST, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response).await;
    let protections = json["protections"].as_array().unwrap();
    assert_eq!(protections.len(), 2);
    assert_eq!(protections[0]["protection"], "email");
    assert_eq!(protections[0]["provider"], "smtp");
    assert_eq!(protections[1]["protection"], "bot");
    assert_eq!(protections[1]["provider"], "turnstile");
    for protection in protections {
        assert_eq!(protection["enabled"], false);
        assert_eq!(protection["status"], "configuration_invalid");
        assert_eq!(protection["reason"], "not_configured");
        assert!(protection["generation"].as_str().unwrap().len() >= 16);
        assert!(protection["last_check"].is_null());
    }
    let text = json.to_string();
    for forbidden in ["secret", "password", "token", "proof", "@"] {
        assert!(
            !text.contains(forbidden),
            "{forbidden} must not appear: {text}"
        );
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn verify_runs_a_real_check_records_both_business_result_and_transport_and_validates_input() {
    let _guard = crate::route::registration_gate::test_support::GATE_ROUTE_TEST_LOCK
        .lock()
        .await;
    let keto = MockServer::start().await;
    let app = app(&keto).await;

    // 未設定のまま検証しても operational にはならない: configuration_invalid が記録される。
    let response = app
        .router
        .clone()
        .oneshot(request(
            Some(&app.admin_a),
            Method::POST,
            &format!("{LIST}/bot/verify"),
            Some(serde_json::json!({})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response).await;
    assert_eq!(json["result"]["outcome"], "configuration_invalid");
    assert_eq!(json["result"]["reason"], "not_configured");
    assert_eq!(json["result"]["transport"], "none");
    assert_eq!(json["recorded"], true);
    assert_eq!(json["disposition"], "completed");
    assert_eq!(json["readiness"]["status"], "configuration_invalid");
    assert_eq!(
        json["readiness"]["last_check"]["outcome"],
        "configuration_invalid"
    );

    let response = app
        .router
        .clone()
        .oneshot(request(
            Some(&app.admin_a),
            Method::POST,
            &format!("{LIST}/email/verify"),
            Some(serde_json::json!({})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response).await;
    assert_eq!(json["result"]["outcome"], "configuration_invalid");
    assert_eq!(json["disposition"], "completed");

    // 入力検証: 不正な組み合わせ・不正な body・未知の protection。
    for (protection, body, expected) in [
        (
            "bot",
            serde_json::json!({"code": "1"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "bot",
            serde_json::json!({"token": "  "}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "email",
            serde_json::json!({"token": "t"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "email",
            serde_json::json!({"challenge_id": "x"}),
            StatusCode::BAD_REQUEST,
        ),
        ("sms", serde_json::json!({}), StatusCode::NOT_FOUND),
    ] {
        let response = app
            .router
            .clone()
            .oneshot(request(
                Some(&app.admin_a),
                Method::POST,
                &format!("{LIST}/{protection}/verify"),
                Some(body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{protection} {body}");
    }
    let response = app
        .router
        .clone()
        .oneshot(request(
            Some(&app.admin_a),
            Method::POST,
            &format!("{LIST}/bot/verify"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // 記録された検証結果は一覧に反映される。
    let response = app
        .router
        .clone()
        .oneshot(request(Some(&app.admin_a), Method::GET, LIST, None))
        .await
        .unwrap();
    let json = response_json(response).await;
    assert_eq!(
        json["protections"][1]["last_check"]["outcome"],
        "configuration_invalid"
    );
    assert_eq!(json["protections"][1]["last_check"]["transport"], "none");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn public_email_challenge_endpoints_are_unauthenticated_validated_and_do_not_send_when_unavailable(
) {
    let _guard = crate::route::registration_gate::test_support::GATE_ROUTE_TEST_LOCK
        .lock()
        .await;
    let keto = MockServer::start().await;
    let app = app(&keto).await;

    // 不正な email / flow / body は 400 (認証は要求されない = 401 ではない)。
    for body in [
        Some(serde_json::json!({"email": "not-an-email", "flow_id": "flow-1"})),
        Some(serde_json::json!({"email": "a@example.com,b@example.com", "flow_id": "flow-1"})),
        Some(serde_json::json!({"email": "a@example.com", "flow_id": "bad flow!"})),
        Some(serde_json::json!({"email": "a@example.com"})),
        Some(serde_json::json!({})),
        None,
    ] {
        let response = app
            .router
            .clone()
            .oneshot(request(None, Method::POST, ISSUE, body.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body:?}");
    }

    // 保護が operational でなければ、任意の宛先へ送らず 503 (宛先の情報を含まない)。
    let response = app
        .router
        .clone()
        .oneshot(request(
            None,
            Method::POST,
            ISSUE,
            Some(serde_json::json!({"email": "victim@example.com", "flow_id": "flow-1"})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        !text.contains("victim"),
        "the response never echoes the recipient: {text}"
    );

    // 確認: 不明な challenge・不正な body は区別せず 400。
    for (id, body) in [
        (
            "0123456789abcdef0123456789abcdef",
            Some(serde_json::json!({"code": "12345678"})),
        ),
        ("unknown", Some(serde_json::json!({"code": "12345678"}))),
        (
            "0123456789abcdef0123456789abcdef",
            Some(serde_json::json!({})),
        ),
        ("0123456789abcdef0123456789abcdef", None),
    ] {
        let response = app
            .router
            .clone()
            .oneshot(request(
                None,
                Method::POST,
                &format!("{ISSUE}/{id}/confirm"),
                body.clone(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{id} {body:?}");
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn proof_verification_and_consumption_are_not_exposed_over_http() {
    let _guard = crate::route::registration_gate::test_support::GATE_ROUTE_TEST_LOCK
        .lock()
        .await;
    let keto = MockServer::start().await;
    let app = app(&keto).await;
    for uri in [
        "/api/v1/registration/email-proofs",
        "/api/v1/registration/email-proofs/verify",
        "/api/v1/registration/email-proofs/consume",
        "/api/v1/registration/email-challenges/abc/verify",
        "/api/v1/registration/email-challenges/abc/consume",
        "/internal/v1/registration/email-proofs/verify",
    ] {
        for token in [None, Some(&app.admin_a)] {
            let response = app
                .router
                .clone()
                .oneshot(request(
                    token,
                    Method::POST,
                    uri,
                    Some(serde_json::json!({"proof": "x"})),
                ))
                .await
                .unwrap();
            assert!(
                matches!(
                    response.status(),
                    StatusCode::NOT_FOUND
                        | StatusCode::METHOD_NOT_ALLOWED
                        | StatusCode::UNAUTHORIZED
                ),
                "{uri} must not be a write entry: {}",
                response.status()
            );
            assert_ne!(response.status(), StatusCode::OK, "{uri}");
        }
    }
}

#[test]
fn openapi_documents_public_and_admin_registration_protection_endpoints() {
    let spec: serde_json::Value =
        serde_json::from_str(&crate::openapi::generate_openapi_json()).unwrap();
    for (path, method, secured) in [
        ("/api/v1/registration/email-challenges", "post", false),
        (
            "/api/v1/registration/email-challenges/{challenge_id}/confirm",
            "post",
            false,
        ),
        ("/api/v1/admin/registration-gate/protections", "get", true),
        (
            "/api/v1/admin/registration-gate/protections/{protection}/verify",
            "post",
            true,
        ),
    ] {
        let operation = &spec["paths"][path][method];
        assert!(operation.is_object(), "{method} {path} must be registered");
        assert_eq!(
            operation.get("security").is_some(),
            secured,
            "{method} {path} security"
        );
    }
    let paths = spec["paths"].as_object().unwrap();
    assert!(
        !paths.keys().any(|p| p.contains("email-proof")),
        "proof verification is an internal use case, not an HTTP endpoint"
    );
    let schemas = spec["components"]["schemas"].as_object().unwrap();
    for name in [
        "ProtectionReadinessResponse",
        "VerifyProtectionResponse",
        "EmailProofResponse",
    ] {
        assert!(schemas.contains_key(name), "{name}");
    }
    // 管理者の応答に secret・token・メールアドレスを持つフィールドはない。
    for name in [
        "ProtectionReadinessResponse",
        "VerifyProtectionResponse",
        "LastCheckResponse",
    ] {
        let properties = schemas[name]["properties"].as_object().unwrap();
        for forbidden in [
            "secret",
            "password",
            "token",
            "proof",
            "email_address",
            "email",
        ] {
            assert!(
                !properties.keys().any(|key| key == forbidden),
                "{name} must not expose {forbidden}"
            );
        }
    }
}

mod flow_directory {
    use crate::kratos::KratosRegistrationFlowDirectory;
    use kernel::interfaces::repository::{RegistrationFlowDirectory, RegistrationFlowStatus};
    use time::format_description::well_known::Rfc3339;
    use time::{Duration, OffsetDateTime};
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn kratos(response: ResponseTemplate) -> (MockServer, KratosRegistrationFlowDirectory) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/self-service/registration/flows"))
            .and(query_param("id", "flow-1"))
            .respond_with(response)
            .mount(&server)
            .await;
        let directory = KratosRegistrationFlowDirectory::new(&server.uri());
        (server, directory)
    }

    fn flow_body(expires_in: Duration) -> (OffsetDateTime, serde_json::Value) {
        let expires = OffsetDateTime::now_utc() + expires_in;
        (
            expires,
            serde_json::json!({"id": "flow-1", "expires_at": expires.format(&Rfc3339).unwrap()}),
        )
    }

    #[tokio::test]
    async fn an_active_flow_returns_its_real_expiry() {
        let (expires, body) = flow_body(Duration::minutes(30));
        let (_server, directory) = kratos(ResponseTemplate::new(200).set_body_json(body)).await;
        match directory.lookup("flow-1", None).await.unwrap() {
            RegistrationFlowStatus::Active { expires_at } => {
                assert!((expires_at - expires).abs() < Duration::seconds(1));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn expired_missing_or_gone_flows_are_not_usable() {
        let (_, body) = flow_body(Duration::minutes(-1));
        let (_server, directory) = kratos(ResponseTemplate::new(200).set_body_json(body)).await;
        assert_eq!(
            directory.lookup("flow-1", None).await.unwrap(),
            RegistrationFlowStatus::NotUsable
        );
        for status in [404, 410] {
            let (_server, directory) = kratos(ResponseTemplate::new(status)).await;
            assert_eq!(
                directory.lookup("flow-1", None).await.unwrap(),
                RegistrationFlowStatus::NotUsable,
                "{status}"
            );
        }
    }

    #[tokio::test]
    async fn a_browser_flow_is_checked_with_its_csrf_cookie_and_keeps_its_short_real_expiry() {
        // 残存期間の短い browser flow: 合成した期限ではなく Kratos が返した実際の期限を使う。
        let (expires, body) = flow_body(Duration::seconds(90));
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/self-service/registration/flows"))
            .and(header("cookie", "csrf_token_abc=value1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/self-service/registration/flows"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let directory = KratosRegistrationFlowDirectory::new(&server.uri());
        match directory
            .lookup("flow-1", Some("csrf_token_abc=value1"))
            .await
            .unwrap()
        {
            RegistrationFlowStatus::Active { expires_at } => {
                assert!((expires_at - expires).abs() < Duration::seconds(1));
                assert!(expires_at - OffsetDateTime::now_utc() <= Duration::seconds(90));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_browser_flow_that_cannot_be_inspected_is_rejected_not_guessed() {
        // cookie なし・不一致の cookie はどちらも 403。期限を確認できない flow は使えない (fail-closed)。
        let (_server, directory) = kratos(ResponseTemplate::new(403)).await;
        assert_eq!(
            directory.lookup("flow-1", None).await.unwrap(),
            RegistrationFlowStatus::NotUsable
        );
        assert_eq!(
            directory
                .lookup("flow-1", Some("csrf_token_abc=wrong"))
                .await
                .unwrap(),
            RegistrationFlowStatus::NotUsable
        );
    }

    #[tokio::test]
    async fn kratos_trouble_is_an_error_so_that_nothing_is_sent() {
        for response in [
            ResponseTemplate::new(500),
            ResponseTemplate::new(200).set_body_string("not json"),
            ResponseTemplate::new(200).set_body_json(serde_json::json!({})),
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"expires_at": "2999-01-01T00:00:00Z"}))
                .set_delay(std::time::Duration::from_secs(5)),
        ] {
            let (_server, directory) = kratos(response).await;
            assert!(directory.lookup("flow-1", None).await.is_err());
        }
        let unreachable = KratosRegistrationFlowDirectory::new("http://127.0.0.1:1");
        assert!(unreachable.lookup("flow-1", None).await.is_err());
    }
}
