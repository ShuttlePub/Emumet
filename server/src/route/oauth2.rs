use crate::handler::AppModule;
use crate::hydra::{AcceptConsentRequest, AcceptLoginRequest, RejectRequest};
use crate::kratos::KratosClient;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const REMEMBER_FOR_SECS: i64 = 3600;

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LoginQuery {
    login_challenge: String,
}

#[derive(Debug, Deserialize)]
struct ConsentQuery {
    consent_challenge: String,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(tag = "action")]
enum OAuth2Response {
    #[serde(rename = "redirect")]
    Redirect { redirect_to: String },
    #[serde(rename = "show_consent")]
    ShowConsent {
        consent_challenge: String,
        client_name: Option<String>,
        requested_scope: Vec<String>,
    },
}

#[derive(Debug, Deserialize)]
struct ConsentDecision {
    consent_challenge: String,
    accept: bool,
    grant_scope: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// GET /oauth2/login
// ---------------------------------------------------------------------------

async fn login(
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

    let kratos_session = verify_kratos_session(kratos, cookie).await?;

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

async fn get_consent(
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

async fn post_consent(
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
