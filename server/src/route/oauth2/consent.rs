use super::status_of;
use crate::api::OAuth2Api;
use crate::schema::oauth2::{ConsentDecision, ConsentQuery, OAuth2Response};
use application::service::login_consent::{ConsentDecision as Decision, ConsentOutcome};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use kernel::interfaces::login_consent::ConsentChallenge;

// ---------------------------------------------------------------------------
// GET /oauth2/consent
// ---------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/oauth2/consent",
    description = "Retrieve consent request details or auto-accept if skip is configured.",
    params(("consent_challenge" = String, Query, description = "Hydra consent challenge")),
    responses(
        (status = 200, description = "Consent details for user approval", body = OAuth2Response),
        (status = 302, description = "Redirect (auto-accepted or skipped)"),
        (status = 502, description = "Bad gateway (Hydra error)"),
    ),
    tag = "OAuth2",
)]
pub(crate) async fn get_consent(
    State(api): State<OAuth2Api>,
    Query(ConsentQuery { consent_challenge }): Query<ConsentQuery>,
) -> Result<Response, StatusCode> {
    let outcome = api
        .begin_consent(&ConsentChallenge::new(consent_challenge))
        .await
        .map_err(status_of)?;

    Ok(match outcome {
        ConsentOutcome::Redirect(redirect) => OAuth2Response::Redirect {
            redirect_to: redirect.into_string(),
        },
        ConsentOutcome::Prompt(prompt) => OAuth2Response::ShowConsent {
            consent_challenge: prompt.consent_challenge.as_str().to_string(),
            client_name: prompt.client_name,
            requested_scope: prompt.requested_scope,
        },
    }
    .into_response())
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
        (status = 302, description = "Redirect after consent decision"),
        (status = 400, description = "Invalid scope requested"),
        (status = 502, description = "Bad gateway (Hydra error)"),
    ),
    tag = "OAuth2",
)]
pub(crate) async fn post_consent(
    State(api): State<OAuth2Api>,
    Json(decision): Json<ConsentDecision>,
) -> Result<Response, StatusCode> {
    let redirect = api
        .decide_consent(Decision {
            challenge: ConsentChallenge::new(decision.consent_challenge),
            accept: decision.accept,
            grant_scope: decision.grant_scope,
        })
        .await
        .map_err(status_of)?;

    Ok(OAuth2Response::Redirect {
        redirect_to: redirect.into_string(),
    }
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{assert_redirect, build_app, response_json};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

        assert_redirect(&resp, "http://example.com/token");
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

        assert_redirect(&resp, "http://example.com/token2");
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

        assert_redirect(&resp, "http://example.com/done");
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

        assert_redirect(&resp, "http://example.com/denied");
    }
}
