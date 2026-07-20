use super::REMEMBER_FOR_SECS;
use crate::handler::AppModule;
use crate::hydra::{AcceptConsentRequest, ConsentSession, RejectRequest};
use crate::schema::oauth2::{ConsentDecision, ConsentQuery, OAuth2Response};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::collections::HashSet;

fn build_consent_session(
    context: &serde_json::Value,
    granted_scopes: &[String],
) -> Option<ConsentSession> {
    let mut id_token = serde_json::Map::new();

    if granted_scopes.iter().any(|s| s == "email") {
        if let Some(email) = context.get("email").and_then(|v| v.as_str()) {
            id_token.insert("email".to_string(), serde_json::json!(email));
        }
    }

    if id_token.is_empty() {
        None
    } else {
        Some(ConsentSession {
            id_token: Some(serde_json::Value::Object(id_token)),
            access_token: None,
        })
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
        (status = 200, description = "Consent details for user approval", body = OAuth2Response),
        (status = 302, description = "Redirect (auto-accepted or skipped)"),
        (status = 502, description = "Bad gateway (Hydra error)"),
    ),
    tag = "OAuth2",
)]
pub(crate) async fn get_consent(
    State(module): State<AppModule>,
    Query(ConsentQuery { consent_challenge }): Query<ConsentQuery>,
) -> Result<Response, StatusCode> {
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
                    session: build_consent_session(
                        &consent_request.context,
                        &consent_request.requested_scope,
                    ),
                },
            )
            .await
            .map_err(|e| {
                tracing::error!("Failed to accept consent at Hydra: {e}");
                StatusCode::BAD_GATEWAY
            })?;

        return Ok(OAuth2Response::Redirect {
            redirect_to: redirect.redirect_to,
        }
        .into_response());
    }

    // Non-skip: return consent details for frontend to display.
    let client_name = consent_request
        .client
        .as_ref()
        .and_then(|c| c.client_name.clone());

    Ok(OAuth2Response::ShowConsent {
        consent_challenge,
        client_name,
        requested_scope: consent_request.requested_scope,
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
    State(module): State<AppModule>,
    Json(decision): Json<ConsentDecision>,
) -> Result<Response, StatusCode> {
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

        let session = build_consent_session(&consent_request.context, &grant_scope);
        let redirect = hydra
            .accept_consent(
                &decision.consent_challenge,
                &AcceptConsentRequest {
                    grant_scope,
                    grant_access_token_audience: consent_request.requested_access_token_audience,
                    remember: Some(true),
                    remember_for: Some(REMEMBER_FOR_SECS),
                    session,
                },
            )
            .await
            .map_err(|e| {
                tracing::error!("Failed to accept consent at Hydra: {e}");
                StatusCode::BAD_GATEWAY
            })?;

        Ok(OAuth2Response::Redirect {
            redirect_to: redirect.redirect_to,
        }
        .into_response())
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

        Ok(OAuth2Response::Redirect {
            redirect_to: redirect.redirect_to,
        }
        .into_response())
    }
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
