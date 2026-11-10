use crate::api::AdminRegistrationProtectionApi;
use crate::auth::{AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::schema::registration_protection::{
    ProtectionListResponse, VerifyProtectionRequest, VerifyProtectionResponse,
};
use application::service::registration_protection::AdminVerifyInput;
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
    description = "Run one real verification now (bootstrap or re-verification) and record both the business result and the transport classification. bot: with `token` (a valid token issued by the widget for the configured site key) the real Siteverify business result decides (success must match the expected hostname/action and be fresh); without it only a negative probe runs (confirms the secret is accepted, never makes the protection operational). email: without `challenge_id`/`code` an SMTP connection/auth probe runs and a code is sent to the approved canary mailbox (REGISTRATION_EMAIL_CANARY_ADDRESS; `canary_not_configured` leaves the protection unverified); with `challenge_id` and `code` the canary roundtrip is confirmed, which is the business success. A canary confirmation never issues a registration proof. A user-attributable rejection (`attempt_rejected`, for example a wrong code or an invalid token) is reported but not recorded and does not change the gate observation. Recorded results update the registration gate observation through the E15a use case.",
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
    let request = request
        .map(|Json(request)| request)
        .map_err(|rejection| bad_request(rejection.body_text()))?;
    let admin = resolve_admin(&api, claims).await?;
    let report = api
        .verify(
            &admin,
            protection,
            AdminVerifyInput {
                token: request.token,
                challenge_id: request.challenge_id,
                code: request.code,
            },
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json((&report).into()))
}
