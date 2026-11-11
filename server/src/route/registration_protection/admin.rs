use crate::api::AdminRegistrationProtectionApi;
use crate::auth::{AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::schema::registration_protection::{
    ProtectionListResponse, VerifyProtectionRequest, VerifyProtectionResponse,
};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use kernel::prelude::entity::{AuthAccountId, Protection};

fn bad_request(message: impl Into<String>) -> ErrorStatus {
    ErrorStatus::from((StatusCode::BAD_REQUEST, message.into()))
}

async fn resolve_admin(
    api: &AdminRegistrationProtectionApi,
    claims: AuthClaims,
) -> Result<AuthAccountId, ErrorStatus> {
    api.resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)
}

fn parse_protection(raw: &str) -> Result<Protection, ErrorStatus> {
    match raw {
        "email" => Ok(Protection::Email),
        "bot" => Ok(Protection::Bot),
        _ => Err(ErrorStatus::from(StatusCode::NOT_FOUND)),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/registration-gate/protections",
    description = "Readiness of each registration protection (email via SMTP, bot via Turnstile), evaluated at the current time against the current configuration. `status` is `operational` only while a real verification business success bound to the current configuration `generation` is fresh (`valid_until`); health, HTTP 200 or an SMTP 250 alone never make a protection operational. `unverified` covers never verified, configuration changed, success expired and probe stopped. `configuration_invalid` covers unset/empty/disabled configuration, invalid secrets and SMTP auth/configuration failures; `temporarily_unavailable` covers 429/5xx/timeouts/connection failures/SMTP 4xx. Secrets, tokens, proofs and registrant emails never appear; `fingerprint` is a short non-reversible digest of the secret.",
    responses(
        (status = 200, description = "Protection readiness", body = ProtectionListResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationProtection",
)]
pub(crate) async fn list_registration_protections(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationProtectionApi>,
) -> Result<Json<ProtectionListResponse>, ErrorStatus> {
    let admin = resolve_admin(&api, claims).await?;
    let views = api.list(&admin).await.map_err(ErrorStatus::from)?;
    Ok(Json(ProtectionListResponse {
        protections: views.iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/registration-gate/protections/{protection}/verify",
    description = "Run one real verification now (initial bootstrap or re-verification) with no input, and record both the business result and the transport classification. The request body is an empty object `{}`; unknown fields (the removed `token` / `challenge_id` / `code`) are rejected with 400. A protection becomes operational only through this administrator verification (configuration alone never does; the periodic worker only renews a generation already verified here). bot: Siteverify is called with a deliberately invalid probe token; a user-token rejection (`invalid-input-response`) proves the secret is valid and is the business success, `invalid-input-secret` / `missing-input-secret` are `configuration_invalid`, and 429/5xx/timeouts/malformed responses are `temporarily_unavailable` (HTTP 200 alone is never success). email: a code is really sent to the fixed approved canary mailbox (REGISTRATION_EMAIL_CANARY_ADDRESS) and SMTP DATA acceptance is the business success; failures at CONNECT/STARTTLS/AUTH/MAIL FROM/RCPT/DATA are classified and recorded; `canary_not_configured` leaves the protection unverified. No code confirmation exists and no registration proof is issued. Responses always carry `pending_challenge: null`. Recorded results update the registration gate observation through the E15a use case.",
    params(("protection" = String, Path, description = "`email` or `bot`")),
    request_body = VerifyProtectionRequest,
    responses(
        (status = 200, description = "Verification result", body = VerifyProtectionResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "Unknown protection"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationProtection",
)]
pub(crate) async fn verify_registration_protection(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationProtectionApi>,
    Path(protection): Path<String>,
    request: Result<Json<VerifyProtectionRequest>, JsonRejection>,
) -> Result<Json<VerifyProtectionResponse>, ErrorStatus> {
    let protection = parse_protection(&protection)?;
    // 本文は空のオブジェクトだけ (未知のフィールドは拒否)。入力は検証に使わない。
    let Json(_empty) = request.map_err(|rejection| bad_request(rejection.body_text()))?;
    let admin = resolve_admin(&api, claims).await?;
    let report = api
        .verify(&admin, protection)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json((&report).into()))
}
