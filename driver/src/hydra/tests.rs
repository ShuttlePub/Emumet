use super::*;
use serde_json::json;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const UNREACHABLE: &str = "http://127.0.0.1:1";

fn login() -> LoginChallenge {
    LoginChallenge::new("login-challenge")
}

fn consent() -> ConsentChallenge {
    ConsentChallenge::new("consent-challenge")
}

fn rejection() -> Rejection {
    Rejection {
        error: "login_required".to_string(),
        error_description: Some("No valid Kratos session found.".to_string()),
    }
}

fn accept_login(context: Option<LoginContext>) -> AcceptLogin {
    AcceptLogin {
        subject: "identity-1".to_string(),
        remember: true,
        remember_for_secs: 3600,
        context,
    }
}

fn accept_consent(id_token_email: Option<&str>) -> AcceptConsent {
    AcceptConsent {
        grant_scope: vec!["openid".to_string(), "email".to_string()],
        grant_access_token_audience: vec!["account".to_string()],
        remember: true,
        remember_for_secs: 3600,
        id_token_email: id_token_email.map(String::from),
    }
}

fn login_request_body() -> serde_json::Value {
    json!({
        "challenge": "login-challenge",
        "skip": true,
        "subject": "user-uuid",
        "client": null,
        "requested_scope": ["openid"],
        "requested_access_token_audience": ["account"],
        "request_url": "http://example.com"
    })
}

fn redirect_body(url: &str) -> serde_json::Value {
    json!({ "redirect_to": url })
}

async fn mount_get(server: &MockServer, route: &str, key: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(route))
        .and(query_param(key, "login-challenge"))
        .respond_with(response)
        .mount(server)
        .await;
}

// ---------------------------------------------------------------------------
// get_login_request
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_request_is_mapped_to_the_boundary_type() {
    let server = MockServer::start().await;
    mount_get(
        &server,
        "/admin/oauth2/auth/requests/login",
        "login_challenge",
        ResponseTemplate::new(200).set_body_json(login_request_body()),
    )
    .await;

    let request = HydraAdminClient::new(server.uri())
        .get_login_request(&login())
        .await
        .unwrap();

    assert_eq!(
        request,
        LoginRequest {
            skip: true,
            subject: "user-uuid".to_string()
        }
    );
}

#[tokio::test]
async fn challenge_is_url_encoded_in_the_query() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/admin/oauth2/auth/requests/login"))
        .and(query_param("login_challenge", "a b&c=d"))
        .respond_with(ResponseTemplate::new(200).set_body_json(login_request_body()))
        .expect(1)
        .mount(&server)
        .await;

    HydraAdminClient::new(server.uri())
        .get_login_request(&LoginChallenge::new("a b&c=d"))
        .await
        .unwrap();
}

#[tokio::test]
async fn trailing_slash_of_the_admin_url_is_ignored() {
    let server = MockServer::start().await;
    mount_get(
        &server,
        "/admin/oauth2/auth/requests/login",
        "login_challenge",
        ResponseTemplate::new(200).set_body_json(login_request_body()),
    )
    .await;

    HydraAdminClient::new(format!("{}/", server.uri()))
        .get_login_request(&login())
        .await
        .unwrap();
}

#[tokio::test]
async fn login_request_hydra_4xx_is_a_status_error() {
    for status in [400_u16, 404, 409, 410] {
        let server = MockServer::start().await;
        mount_get(
            &server,
            "/admin/oauth2/auth/requests/login",
            "login_challenge",
            ResponseTemplate::new(status).set_body_json(json!({ "error": "x" })),
        )
        .await;

        let error = HydraAdminClient::new(server.uri())
            .get_login_request(&login())
            .await
            .unwrap_err();

        assert_eq!(error.kind(), UpstreamErrorKind::Status(status));
    }
}

#[tokio::test]
async fn login_request_hydra_5xx_is_a_status_error() {
    let server = MockServer::start().await;
    mount_get(
        &server,
        "/admin/oauth2/auth/requests/login",
        "login_challenge",
        ResponseTemplate::new(503),
    )
    .await;

    let error = HydraAdminClient::new(server.uri())
        .get_login_request(&login())
        .await
        .unwrap_err();

    assert_eq!(error.kind(), UpstreamErrorKind::Status(503));
}

#[tokio::test]
async fn login_request_with_a_missing_required_field_is_an_invalid_response() {
    let server = MockServer::start().await;
    let mut body = login_request_body();
    body.as_object_mut().unwrap().remove("request_url");
    mount_get(
        &server,
        "/admin/oauth2/auth/requests/login",
        "login_challenge",
        ResponseTemplate::new(200).set_body_json(body),
    )
    .await;

    let error = HydraAdminClient::new(server.uri())
        .get_login_request(&login())
        .await
        .unwrap_err();

    assert_eq!(error.kind(), UpstreamErrorKind::InvalidResponse);
}

#[tokio::test]
async fn login_request_with_a_non_json_body_is_an_invalid_response() {
    let server = MockServer::start().await;
    mount_get(
        &server,
        "/admin/oauth2/auth/requests/login",
        "login_challenge",
        ResponseTemplate::new(200).set_body_string("not json"),
    )
    .await;

    let error = HydraAdminClient::new(server.uri())
        .get_login_request(&login())
        .await
        .unwrap_err();

    assert_eq!(error.kind(), UpstreamErrorKind::InvalidResponse);
}

#[tokio::test]
async fn login_request_connection_failure_is_unreachable() {
    let error = HydraAdminClient::new(UNREACHABLE.to_string())
        .get_login_request(&login())
        .await
        .unwrap_err();

    assert_eq!(error.kind(), UpstreamErrorKind::Unreachable);
}

// ---------------------------------------------------------------------------
// accept_login / reject_login
// ---------------------------------------------------------------------------

#[tokio::test]
async fn accept_login_sends_subject_remember_and_email_context() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/admin/oauth2/auth/requests/login/accept"))
        .and(query_param("login_challenge", "login-challenge"))
        .and(body_json(json!({
            "subject": "identity-1",
            "remember": true,
            "remember_for": 3600,
            "context": { "email": "a@example.com" }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(redirect_body("http://hydra/next")))
        .expect(1)
        .mount(&server)
        .await;

    let redirect = HydraAdminClient::new(server.uri())
        .accept_login(
            &login(),
            &accept_login(Some(LoginContext {
                email: Some("a@example.com".to_string()),
            })),
        )
        .await
        .unwrap();

    assert_eq!(redirect, RedirectTarget::new("http://hydra/next"));
}

#[tokio::test]
async fn accept_login_without_context_omits_the_context() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/admin/oauth2/auth/requests/login/accept"))
        .and(body_json(json!({
            "subject": "identity-1",
            "remember": true,
            "remember_for": 3600
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(redirect_body("http://hydra/next")))
        .expect(2)
        .mount(&server)
        .await;
    let client = HydraAdminClient::new(server.uri());

    client
        .accept_login(&login(), &accept_login(None))
        .await
        .unwrap();
    client
        .accept_login(&login(), &accept_login(Some(LoginContext { email: None })))
        .await
        .unwrap();
}

#[tokio::test]
async fn reject_login_sends_error_and_description() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/admin/oauth2/auth/requests/login/reject"))
        .and(query_param("login_challenge", "login-challenge"))
        .and(body_json(json!({
            "error": "login_required",
            "error_description": "No valid Kratos session found."
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(redirect_body("http://hydra/rej")))
        .expect(1)
        .mount(&server)
        .await;

    let redirect = HydraAdminClient::new(server.uri())
        .reject_login(&login(), &rejection())
        .await
        .unwrap();

    assert_eq!(redirect, RedirectTarget::new("http://hydra/rej"));
}

#[tokio::test]
async fn put_failures_are_classified() {
    // 4xx / 5xx
    for status in [400_u16, 404, 500] {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        let client = HydraAdminClient::new(server.uri());

        let accept = client
            .accept_login(&login(), &accept_login(None))
            .await
            .unwrap_err();
        let reject = client
            .reject_login(&login(), &rejection())
            .await
            .unwrap_err();

        assert_eq!(accept.kind(), UpstreamErrorKind::Status(status));
        assert_eq!(reject.kind(), UpstreamErrorKind::Status(status));
    }

    // a 200 without redirect_to
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    let error = HydraAdminClient::new(server.uri())
        .accept_login(&login(), &accept_login(None))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), UpstreamErrorKind::InvalidResponse);

    // connection failure
    let error = HydraAdminClient::new(UNREACHABLE.to_string())
        .reject_login(&login(), &rejection())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), UpstreamErrorKind::Unreachable);
}

// ---------------------------------------------------------------------------
// consent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn consent_request_is_mapped_with_client_and_login_email() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/admin/oauth2/auth/requests/consent"))
        .and(query_param("consent_challenge", "consent-challenge"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "challenge": "consent-challenge",
            "skip": false,
            "subject": "user-uuid",
            "client": {
                "client_id": "my-app",
                "client_name": "My App",
                "skip_consent": true
            },
            "requested_scope": ["openid", "email"],
            "requested_access_token_audience": ["account"],
            "context": { "email": "a@example.com", "other": 1 }
        })))
        .mount(&server)
        .await;

    let request = HydraAdminClient::new(server.uri())
        .get_consent_request(&consent())
        .await
        .unwrap();

    assert_eq!(
        request,
        ConsentRequest {
            skip: false,
            client: Some(ConsentClient {
                client_name: Some("My App".to_string()),
                skip_consent: Some(true),
            }),
            requested_scope: vec!["openid".to_string(), "email".to_string()],
            requested_access_token_audience: vec!["account".to_string()],
            context: LoginContext {
                email: Some("a@example.com".to_string())
            },
        }
    );
}

#[tokio::test]
async fn consent_request_without_context_or_client_has_no_email() {
    for context in [
        None,
        Some(json!(null)),
        Some(json!("text")),
        Some(json!({"email": 5})),
    ] {
        let server = MockServer::start().await;
        let mut body = json!({
            "challenge": "consent-challenge",
            "skip": true,
            "subject": "user-uuid",
            "client": null,
            "requested_scope": ["openid"],
            "requested_access_token_audience": []
        });
        if let Some(context) = context {
            body["context"] = context;
        }
        Mock::given(method("GET"))
            .and(path("/admin/oauth2/auth/requests/consent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;

        let request = HydraAdminClient::new(server.uri())
            .get_consent_request(&consent())
            .await
            .unwrap();

        assert!(request.skip);
        assert_eq!(request.client, None);
        assert_eq!(request.context, LoginContext { email: None });
    }
}

#[tokio::test]
async fn consent_request_failures_are_classified() {
    for status in [404_u16, 500] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        let error = HydraAdminClient::new(server.uri())
            .get_consent_request(&consent())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), UpstreamErrorKind::Status(status));
    }

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "skip": true })))
        .mount(&server)
        .await;
    let error = HydraAdminClient::new(server.uri())
        .get_consent_request(&consent())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), UpstreamErrorKind::InvalidResponse);

    let error = HydraAdminClient::new(UNREACHABLE.to_string())
        .get_consent_request(&consent())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), UpstreamErrorKind::Unreachable);
}

#[tokio::test]
async fn accept_consent_sends_scope_audience_remember_and_email_claim() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/admin/oauth2/auth/requests/consent/accept"))
        .and(query_param("consent_challenge", "consent-challenge"))
        .and(body_json(json!({
            "grant_scope": ["openid", "email"],
            "grant_access_token_audience": ["account"],
            "remember": true,
            "remember_for": 3600,
            "session": { "id_token": { "email": "a@example.com" } }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(redirect_body("http://hydra/tok")))
        .expect(1)
        .mount(&server)
        .await;

    let redirect = HydraAdminClient::new(server.uri())
        .accept_consent(&consent(), &accept_consent(Some("a@example.com")))
        .await
        .unwrap();

    assert_eq!(redirect, RedirectTarget::new("http://hydra/tok"));
}

#[tokio::test]
async fn accept_consent_without_email_claim_omits_the_session() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/admin/oauth2/auth/requests/consent/accept"))
        .and(body_json(json!({
            "grant_scope": ["openid", "email"],
            "grant_access_token_audience": ["account"],
            "remember": true,
            "remember_for": 3600
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(redirect_body("http://hydra/tok")))
        .expect(1)
        .mount(&server)
        .await;

    HydraAdminClient::new(server.uri())
        .accept_consent(&consent(), &accept_consent(None))
        .await
        .unwrap();
}

#[tokio::test]
async fn reject_consent_sends_error_and_description() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/admin/oauth2/auth/requests/consent/reject"))
        .and(query_param("consent_challenge", "consent-challenge"))
        .and(body_json(json!({
            "error": "consent_denied",
            "error_description": "The user denied the consent request."
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(redirect_body("http://hydra/den")))
        .expect(1)
        .mount(&server)
        .await;

    let redirect = HydraAdminClient::new(server.uri())
        .reject_consent(
            &consent(),
            &Rejection {
                error: "consent_denied".to_string(),
                error_description: Some("The user denied the consent request.".to_string()),
            },
        )
        .await
        .unwrap();

    assert_eq!(redirect, RedirectTarget::new("http://hydra/den"));
}

#[tokio::test]
async fn consent_put_failures_are_classified() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(409))
        .mount(&server)
        .await;
    let client = HydraAdminClient::new(server.uri());

    let accept = client
        .accept_consent(&consent(), &accept_consent(None))
        .await
        .unwrap_err();
    let reject = client
        .reject_consent(
            &consent(),
            &Rejection {
                error: "consent_denied".to_string(),
                error_description: None,
            },
        )
        .await
        .unwrap_err();

    assert_eq!(accept.kind(), UpstreamErrorKind::Status(409));
    assert_eq!(reject.kind(), UpstreamErrorKind::Status(409));

    let error = HydraAdminClient::new(UNREACHABLE.to_string())
        .accept_consent(&consent(), &accept_consent(None))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), UpstreamErrorKind::Unreachable);
}

#[test]
#[should_panic(expected = "HYDRA_ADMIN_URL is not a valid URL")]
fn invalid_admin_url_panics_at_construction() {
    let _ = HydraAdminClient::new("not a url".to_string());
}
