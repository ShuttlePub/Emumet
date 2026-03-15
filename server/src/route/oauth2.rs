use crate::handler::AppModule;
use crate::hydra::{AcceptConsentRequest, AcceptLoginRequest, RejectRequest};
use crate::kratos::KratosClient;
use crate::schema::oauth2::{ConsentDecision, ConsentQuery, LoginQuery, OAuth2Response};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use std::collections::HashSet;

const REMEMBER_FOR_SECS: i64 = 3600;

// ---------------------------------------------------------------------------
// GET /oauth2/login
// ---------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/oauth2/login",
    description = "Handle OAuth2 login flow. Verifies Kratos session and accepts Hydra login.",
    params(("login_challenge" = String, Query, description = "Hydra login challenge")),
    responses(
        (status = 200, description = "Login result (redirect on success or reject)", body = OAuth2Response),
        (status = 502, description = "Bad gateway (Hydra/Kratos error)"),
    ),
    tag = "OAuth2",
)]
pub(crate) async fn login(
    State(module): State<AppModule>,
    Query(LoginQuery { login_challenge }): Query<LoginQuery>,
    headers: axum::http::HeaderMap,
) -> Result<Json<OAuth2Response>, StatusCode> {
    let hydra = module.hydra_admin_client();
    let kratos = module.kratos_client();

    // 1. Fetch login request from Hydra.
    let login_request = hydra
        .get_login_request(&login_challenge)
        .await
        .map_err(|e| {
            tracing::error!("Failed to get login request from Hydra: {e}");
            StatusCode::BAD_GATEWAY
        })?;

    // 2. If Hydra says skip (already authenticated), accept immediately.
    if login_request.skip {
        let redirect = hydra
            .accept_login(
                &login_challenge,
                &AcceptLoginRequest {
                    subject: login_request.subject.clone(),
                    remember: Some(true),
                    remember_for: Some(REMEMBER_FOR_SECS),
                },
            )
            .await
            .map_err(|e| {
                tracing::error!("Failed to accept login at Hydra: {e}");
                StatusCode::BAD_GATEWAY
            })?;

        return Ok(Json(OAuth2Response::Redirect {
            redirect_to: redirect.redirect_to,
        }));
    }

    // 3. Verify user has a valid Kratos session via cookie.
    let cookie = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let kratos_session = match verify_kratos_session(kratos, cookie).await {
        Ok(session) => session,
        Err(_) => {
            let redirect = hydra
                .reject_login(
                    &login_challenge,
                    &RejectRequest {
                        error: "login_required".to_string(),
                        error_description: Some("No valid Kratos session found.".to_string()),
                    },
                )
                .await
                .map_err(|e| {
                    tracing::error!("Failed to reject login at Hydra: {e}");
                    StatusCode::BAD_GATEWAY
                })?;

            return Ok(Json(OAuth2Response::Redirect {
                redirect_to: redirect.redirect_to,
            }));
        }
    };

    // 4. Accept login with Kratos identity UUID as subject.
    let redirect = hydra
        .accept_login(
            &login_challenge,
            &AcceptLoginRequest {
                subject: kratos_session.identity_id,
                remember: Some(true),
                remember_for: Some(REMEMBER_FOR_SECS),
            },
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to accept login at Hydra: {e}");
            StatusCode::BAD_GATEWAY
        })?;

    Ok(Json(OAuth2Response::Redirect {
        redirect_to: redirect.redirect_to,
    }))
}

struct VerifiedSession {
    identity_id: String,
}

/// Verify Kratos session via cookie, returning the identity ID on success.
async fn verify_kratos_session(
    kratos: &KratosClient,
    cookie: &str,
) -> Result<VerifiedSession, StatusCode> {
    if cookie.is_empty() {
        tracing::warn!("oauth2/login: no cookie header, cannot verify Kratos session");
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Extract only the Kratos session cookie to avoid leaking other cookies.
    let kratos_cookie = cookie
        .split(';')
        .map(|c| c.trim())
        .find(|c| c.starts_with("ory_kratos_session="))
        .unwrap_or("");

    if kratos_cookie.is_empty() {
        tracing::warn!("oauth2/login: no ory_kratos_session cookie found");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let session = kratos.whoami(kratos_cookie).await.map_err(|e| {
        tracing::error!("Kratos whoami request failed: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    match session {
        Some(s) => Ok(VerifiedSession {
            identity_id: s.identity.id,
        }),
        None => {
            tracing::warn!("oauth2/login: Kratos session invalid or expired");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

// ---------------------------------------------------------------------------
// GET /oauth2/consent
// ---------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/oauth2/consent",
    description = "Retrieve consent request details or auto-accept if skip is configured.",
    params(("consent_challenge" = String, Query, description = "Hydra consent challenge")),
    responses(
        (status = 200, description = "Consent result", body = OAuth2Response),
        (status = 502, description = "Bad gateway (Hydra error)"),
    ),
    tag = "OAuth2",
)]
pub(crate) async fn get_consent(
    State(module): State<AppModule>,
    Query(ConsentQuery { consent_challenge }): Query<ConsentQuery>,
) -> Result<Json<OAuth2Response>, StatusCode> {
    let hydra = module.hydra_admin_client();

    let consent_request = hydra
        .get_consent_request(&consent_challenge)
        .await
        .map_err(|e| {
            tracing::error!("Failed to get consent request from Hydra: {e}");
            StatusCode::BAD_GATEWAY
        })?;

    // If the client is configured to skip consent, accept automatically.
    let skip_consent = consent_request
        .client
        .as_ref()
        .and_then(|c| c.skip_consent)
        .unwrap_or(false);

    if consent_request.skip || skip_consent {
        let redirect = hydra
            .accept_consent(
                &consent_challenge,
                &AcceptConsentRequest {
                    grant_scope: consent_request.requested_scope.clone(),
                    grant_access_token_audience: consent_request
                        .requested_access_token_audience
                        .clone(),
                    remember: Some(true),
                    remember_for: Some(REMEMBER_FOR_SECS),
                    session: None,
                },
            )
            .await
            .map_err(|e| {
                tracing::error!("Failed to accept consent at Hydra: {e}");
                StatusCode::BAD_GATEWAY
            })?;

        return Ok(Json(OAuth2Response::Redirect {
            redirect_to: redirect.redirect_to,
        }));
    }

    // Non-skip: return consent details for frontend to display.
    let client_name = consent_request
        .client
        .as_ref()
        .and_then(|c| c.client_name.clone());

    Ok(Json(OAuth2Response::ShowConsent {
        consent_challenge,
        client_name,
        requested_scope: consent_request.requested_scope,
    }))
}

// ---------------------------------------------------------------------------
// POST /oauth2/consent
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/oauth2/consent",
    description = "Submit consent decision (accept or reject).",
    request_body = ConsentDecision,
    responses(
        (status = 200, description = "Consent decision result", body = OAuth2Response),
        (status = 400, description = "Invalid scope requested"),
        (status = 502, description = "Bad gateway (Hydra error)"),
    ),
    tag = "OAuth2",
)]
pub(crate) async fn post_consent(
    State(module): State<AppModule>,
    Json(decision): Json<ConsentDecision>,
) -> Result<Json<OAuth2Response>, StatusCode> {
    let hydra = module.hydra_admin_client();

    if decision.accept {
        let grant_scope = decision.grant_scope.unwrap_or_default();

        // Re-fetch consent request to get requested_access_token_audience
        // and to validate that granted scopes are a subset of requested scopes.
        let consent_request = hydra
            .get_consent_request(&decision.consent_challenge)
            .await
            .map_err(|e| {
                tracing::error!("Failed to get consent request from Hydra: {e}");
                StatusCode::BAD_GATEWAY
            })?;

        // Validate: grant_scope must be a subset of requested_scope.
        let requested: HashSet<&str> = consent_request
            .requested_scope
            .iter()
            .map(|s| s.as_str())
            .collect();
        for scope in &grant_scope {
            if !requested.contains(scope.as_str()) {
                tracing::warn!("Client attempted to grant unrequested scope: {scope}");
                return Err(StatusCode::BAD_REQUEST);
            }
        }

        let redirect = hydra
            .accept_consent(
                &decision.consent_challenge,
                &AcceptConsentRequest {
                    grant_scope,
                    grant_access_token_audience: consent_request.requested_access_token_audience,
                    remember: Some(true),
                    remember_for: Some(REMEMBER_FOR_SECS),
                    session: None,
                },
            )
            .await
            .map_err(|e| {
                tracing::error!("Failed to accept consent at Hydra: {e}");
                StatusCode::BAD_GATEWAY
            })?;

        Ok(Json(OAuth2Response::Redirect {
            redirect_to: redirect.redirect_to,
        }))
    } else {
        let redirect = hydra
            .reject_consent(
                &decision.consent_challenge,
                &RejectRequest {
                    error: "consent_denied".to_string(),
                    error_description: Some("The user denied the consent request.".to_string()),
                },
            )
            .await
            .map_err(|e| {
                tracing::error!("Failed to reject consent at Hydra: {e}");
                StatusCode::BAD_GATEWAY
            })?;

        Ok(Json(OAuth2Response::Redirect {
            redirect_to: redirect.redirect_to,
        }))
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub trait OAuth2Router {
    fn route_oauth2(self) -> Self;
}

impl OAuth2Router for Router<AppModule> {
    fn route_oauth2(self) -> Self {
        self.route("/oauth2/login", get(login))
            .route("/oauth2/consent", get(get_consent))
            .route("/oauth2/consent", post(post_consent))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn build_app(hydra_url: &str, kratos_url: &str) -> Router {
        let app = AppModule::new_for_oauth2_test(hydra_url.into(), kratos_url.into())
            .await
            .unwrap();
        Router::new().route_oauth2().with_state(app)
    }

    async fn response_json(resp: axum::http::Response<Body>) -> serde_json::Value {
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&body).unwrap_or_else(|e| {
            panic!(
                "Failed to parse response as JSON: {e}\nBody: {}",
                String::from_utf8_lossy(&body)
            )
        })
    }

    // -----------------------------------------------------------------------
    // GET /oauth2/login
    // -----------------------------------------------------------------------

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn login_skip_returns_redirect() {
        let hydra_mock = MockServer::start().await;
        let kratos_mock = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin/oauth2/auth/requests/login"))
            .and(query_param("login_challenge", "test-challenge"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "challenge": "test-challenge",
                "skip": true,
                "subject": "user-uuid",
                "client": null,
                "requested_scope": ["openid"],
                "requested_access_token_audience": ["account"],
                "request_url": "http://example.com"
            })))
            .mount(&hydra_mock)
            .await;

        Mock::given(method("PUT"))
            .and(path("/admin/oauth2/auth/requests/login/accept"))
            .and(query_param("login_challenge", "test-challenge"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "redirect_to": "http://example.com/callback"
            })))
            .mount(&hydra_mock)
            .await;

        let app = build_app(&hydra_mock.uri(), &kratos_mock.uri()).await;

        let resp = app
            .oneshot(
                Request::get("/oauth2/login?login_challenge=test-challenge")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = response_json(resp).await;
        assert_eq!(json["action"], "redirect");
        assert_eq!(json["redirect_to"], "http://example.com/callback");
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn login_valid_kratos_session_returns_redirect() {
        let hydra_mock = MockServer::start().await;
        let kratos_mock = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin/oauth2/auth/requests/login"))
            .and(query_param("login_challenge", "challenge-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "challenge": "challenge-2",
                "skip": false,
                "subject": "",
                "client": null,
                "requested_scope": ["openid"],
                "requested_access_token_audience": ["account"],
                "request_url": "http://example.com"
            })))
            .mount(&hydra_mock)
            .await;

        Mock::given(method("GET"))
            .and(path("/sessions/whoami"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "session-id",
                "active": true,
                "identity": {
                    "id": "identity-uuid",
                    "traits": {}
                }
            })))
            .mount(&kratos_mock)
            .await;

        Mock::given(method("PUT"))
            .and(path("/admin/oauth2/auth/requests/login/accept"))
            .and(query_param("login_challenge", "challenge-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "redirect_to": "http://example.com/consent"
            })))
            .mount(&hydra_mock)
            .await;

        let app = build_app(&hydra_mock.uri(), &kratos_mock.uri()).await;

        let resp = app
            .oneshot(
                Request::get("/oauth2/login?login_challenge=challenge-2")
                    .header("cookie", "ory_kratos_session=test-session-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = response_json(resp).await;
        assert_eq!(json["action"], "redirect");
        assert_eq!(json["redirect_to"], "http://example.com/consent");
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn login_no_cookie_rejects_and_redirects() {
        let hydra_mock = MockServer::start().await;
        let kratos_mock = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin/oauth2/auth/requests/login"))
            .and(query_param("login_challenge", "challenge-3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "challenge": "challenge-3",
                "skip": false,
                "subject": "",
                "client": null,
                "requested_scope": ["openid"],
                "requested_access_token_audience": ["account"],
                "request_url": "http://example.com"
            })))
            .mount(&hydra_mock)
            .await;

        Mock::given(method("PUT"))
            .and(path("/admin/oauth2/auth/requests/login/reject"))
            .and(query_param("login_challenge", "challenge-3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "redirect_to": "http://example.com/login-rejected"
            })))
            .mount(&hydra_mock)
            .await;

        let app = build_app(&hydra_mock.uri(), &kratos_mock.uri()).await;

        let resp = app
            .oneshot(
                Request::get("/oauth2/login?login_challenge=challenge-3")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = response_json(resp).await;
        assert_eq!(json["action"], "redirect");
        assert_eq!(json["redirect_to"], "http://example.com/login-rejected");
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn login_invalid_kratos_session_rejects_and_redirects() {
        let hydra_mock = MockServer::start().await;
        let kratos_mock = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin/oauth2/auth/requests/login"))
            .and(query_param("login_challenge", "challenge-4"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "challenge": "challenge-4",
                "skip": false,
                "subject": "",
                "client": null,
                "requested_scope": ["openid"],
                "requested_access_token_audience": ["account"],
                "request_url": "http://example.com"
            })))
            .mount(&hydra_mock)
            .await;

        Mock::given(method("GET"))
            .and(path("/sessions/whoami"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&kratos_mock)
            .await;

        Mock::given(method("PUT"))
            .and(path("/admin/oauth2/auth/requests/login/reject"))
            .and(query_param("login_challenge", "challenge-4"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "redirect_to": "http://example.com/login-rejected"
            })))
            .mount(&hydra_mock)
            .await;

        let app = build_app(&hydra_mock.uri(), &kratos_mock.uri()).await;

        let resp = app
            .oneshot(
                Request::get("/oauth2/login?login_challenge=challenge-4")
                    .header("cookie", "ory_kratos_session=expired-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = response_json(resp).await;
        assert_eq!(json["action"], "redirect");
        assert_eq!(json["redirect_to"], "http://example.com/login-rejected");
    }

    // -----------------------------------------------------------------------
    // GET /oauth2/consent
    // -----------------------------------------------------------------------

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn consent_skip_returns_redirect() {
        let hydra_mock = MockServer::start().await;
        let kratos_mock = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin/oauth2/auth/requests/consent"))
            .and(query_param("consent_challenge", "consent-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "challenge": "consent-1",
                "skip": true,
                "subject": "user-uuid",
                "client": null,
                "requested_scope": ["openid"],
                "requested_access_token_audience": ["account"]
            })))
            .mount(&hydra_mock)
            .await;

        Mock::given(method("PUT"))
            .and(path("/admin/oauth2/auth/requests/consent/accept"))
            .and(query_param("consent_challenge", "consent-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "redirect_to": "http://example.com/token"
            })))
            .mount(&hydra_mock)
            .await;

        let app = build_app(&hydra_mock.uri(), &kratos_mock.uri()).await;

        let resp = app
            .oneshot(
                Request::get("/oauth2/consent?consent_challenge=consent-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = response_json(resp).await;
        assert_eq!(json["action"], "redirect");
        assert_eq!(json["redirect_to"], "http://example.com/token");
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn consent_client_skip_consent_returns_redirect() {
        let hydra_mock = MockServer::start().await;
        let kratos_mock = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin/oauth2/auth/requests/consent"))
            .and(query_param("consent_challenge", "consent-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "challenge": "consent-2",
                "skip": false,
                "subject": "user-uuid",
                "client": {
                    "client_id": "my-app",
                    "client_name": "My App",
                    "skip_consent": true
                },
                "requested_scope": ["openid", "offline"],
                "requested_access_token_audience": ["account"]
            })))
            .mount(&hydra_mock)
            .await;

        Mock::given(method("PUT"))
            .and(path("/admin/oauth2/auth/requests/consent/accept"))
            .and(query_param("consent_challenge", "consent-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "redirect_to": "http://example.com/token2"
            })))
            .mount(&hydra_mock)
            .await;

        let app = build_app(&hydra_mock.uri(), &kratos_mock.uri()).await;

        let resp = app
            .oneshot(
                Request::get("/oauth2/consent?consent_challenge=consent-2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = response_json(resp).await;
        assert_eq!(json["action"], "redirect");
        assert_eq!(json["redirect_to"], "http://example.com/token2");
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn consent_no_skip_returns_show_consent() {
        let hydra_mock = MockServer::start().await;
        let kratos_mock = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin/oauth2/auth/requests/consent"))
            .and(query_param("consent_challenge", "consent-3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "challenge": "consent-3",
                "skip": false,
                "subject": "user-uuid",
                "client": {
                    "client_id": "my-app",
                    "client_name": "My App",
                    "skip_consent": false
                },
                "requested_scope": ["openid", "profile"],
                "requested_access_token_audience": ["account"]
            })))
            .mount(&hydra_mock)
            .await;

        let app = build_app(&hydra_mock.uri(), &kratos_mock.uri()).await;

        let resp = app
            .oneshot(
                Request::get("/oauth2/consent?consent_challenge=consent-3")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = response_json(resp).await;
        assert_eq!(json["action"], "show_consent");
        assert_eq!(json["consent_challenge"], "consent-3");
        assert_eq!(json["client_name"], "My App");
        assert_eq!(
            json["requested_scope"],
            serde_json::json!(["openid", "profile"])
        );
    }

    // -----------------------------------------------------------------------
    // POST /oauth2/consent
    // -----------------------------------------------------------------------

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn consent_accept_valid_scopes_returns_redirect() {
        let hydra_mock = MockServer::start().await;
        let kratos_mock = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin/oauth2/auth/requests/consent"))
            .and(query_param("consent_challenge", "consent-4"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "challenge": "consent-4",
                "skip": false,
                "subject": "user-uuid",
                "client": null,
                "requested_scope": ["openid", "profile"],
                "requested_access_token_audience": ["account"]
            })))
            .mount(&hydra_mock)
            .await;

        Mock::given(method("PUT"))
            .and(path("/admin/oauth2/auth/requests/consent/accept"))
            .and(query_param("consent_challenge", "consent-4"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "redirect_to": "http://example.com/done"
            })))
            .mount(&hydra_mock)
            .await;

        let app = build_app(&hydra_mock.uri(), &kratos_mock.uri()).await;

        let resp = app
            .oneshot(
                Request::post("/oauth2/consent")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&serde_json::json!({
                            "consent_challenge": "consent-4",
                            "accept": true,
                            "grant_scope": ["openid"]
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = response_json(resp).await;
        assert_eq!(json["action"], "redirect");
        assert_eq!(json["redirect_to"], "http://example.com/done");
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn consent_accept_invalid_scope_returns_400() {
        let hydra_mock = MockServer::start().await;
        let kratos_mock = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin/oauth2/auth/requests/consent"))
            .and(query_param("consent_challenge", "consent-5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "challenge": "consent-5",
                "skip": false,
                "subject": "user-uuid",
                "client": null,
                "requested_scope": ["openid"],
                "requested_access_token_audience": ["account"]
            })))
            .mount(&hydra_mock)
            .await;

        let app = build_app(&hydra_mock.uri(), &kratos_mock.uri()).await;

        let resp = app
            .oneshot(
                Request::post("/oauth2/consent")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&serde_json::json!({
                            "consent_challenge": "consent-5",
                            "accept": true,
                            "grant_scope": ["openid", "admin"]
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn consent_reject_returns_redirect() {
        let hydra_mock = MockServer::start().await;
        let kratos_mock = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/admin/oauth2/auth/requests/consent/reject"))
            .and(query_param("consent_challenge", "consent-6"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "redirect_to": "http://example.com/denied"
            })))
            .mount(&hydra_mock)
            .await;

        let app = build_app(&hydra_mock.uri(), &kratos_mock.uri()).await;

        let resp = app
            .oneshot(
                Request::post("/oauth2/consent")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&serde_json::json!({
                            "consent_challenge": "consent-6",
                            "accept": false,
                            "grant_scope": null
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = response_json(resp).await;
        assert_eq!(json["action"], "redirect");
        assert_eq!(json["redirect_to"], "http://example.com/denied");
    }
}
