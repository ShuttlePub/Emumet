use super::REMEMBER_FOR_SECS;
use crate::handler::AppModule;
use crate::hydra::{AcceptLoginRequest, RejectRequest};
use crate::kratos::KratosClient;
use crate::schema::oauth2::{LoginQuery, OAuth2Response};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

// ---------------------------------------------------------------------------
// GET /oauth2/login
// ---------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/oauth2/login",
    description = "Handle OAuth2 login flow. Verifies Kratos session and accepts Hydra login.",
    params(("login_challenge" = String, Query, description = "Hydra login challenge")),
    responses(
        (status = 302, description = "Redirect to Hydra callback"),
        (status = 502, description = "Bad gateway (Hydra/Kratos error)"),
    ),
    tag = "OAuth2",
)]
pub(crate) async fn login(
    State(module): State<AppModule>,
    Query(LoginQuery { login_challenge }): Query<LoginQuery>,
    headers: axum::http::HeaderMap,
) -> Result<Response, StatusCode> {
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
                    context: None,
                },
            )
            .await
            .map_err(|e| {
                tracing::error!("Failed to accept login at Hydra: {e}");
                StatusCode::BAD_GATEWAY
            })?;

        return Ok(OAuth2Response::Redirect {
            redirect_to: redirect.redirect_to,
        }
        .into_response());
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

            return Ok(OAuth2Response::Redirect {
                redirect_to: redirect.redirect_to,
            }
            .into_response());
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
                context: kratos_session
                    .email
                    .as_ref()
                    .map(|e| serde_json::json!({ "email": e })),
            },
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to accept login at Hydra: {e}");
            StatusCode::BAD_GATEWAY
        })?;

    Ok(OAuth2Response::Redirect {
        redirect_to: redirect.redirect_to,
    }
    .into_response())
}

struct VerifiedSession {
    identity_id: String,
    email: Option<String>,
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
            email: s
                .identity
                .traits
                .get("email")
                .and_then(|v| v.as_str())
                .map(String::from),
        }),
        None => {
            tracing::warn!("oauth2/login: Kratos session invalid or expired");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{assert_redirect, build_app};
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

        assert_redirect(&resp, "http://example.com/callback");
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

        assert_redirect(&resp, "http://example.com/consent");
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

        assert_redirect(&resp, "http://example.com/login-rejected");
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

        assert_redirect(&resp, "http://example.com/login-rejected");
    }
}
