use super::status_of;
use crate::api::OAuth2Api;
use crate::schema::oauth2::{LoginQuery, OAuth2Response};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use kernel::interfaces::login_consent::LoginChallenge;

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
    State(api): State<OAuth2Api>,
    Query(LoginQuery { login_challenge }): Query<LoginQuery>,
    headers: axum::http::HeaderMap,
) -> Result<Response, StatusCode> {
    let cookie_header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let redirect = api
        .login(&LoginChallenge::new(login_challenge), cookie_header)
        .await
        .map_err(status_of)?;

    Ok(OAuth2Response::Redirect {
        redirect_to: redirect.into_string(),
    }
    .into_response())
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
