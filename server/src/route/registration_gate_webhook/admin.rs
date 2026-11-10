use crate::api::AdminRegistrationGateWebhookApi;
use crate::auth::{AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::schema::registration_gate_webhook::{
    WebhookRetryRequest, WebhookRetryResponse, WebhookStatusResponse, WebhookTestResponse,
};
use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use kernel::interfaces::repository::WebhookTestOutcome;
use kernel::prelude::entity::{AuthAccountId, RegistrationGateEventId};

fn bad_request(message: impl Into<String>) -> ErrorStatus {
    ErrorStatus::from((StatusCode::BAD_REQUEST, message.into()))
}

async fn resolve_admin(
    api: &AdminRegistrationGateWebhookApi,
    claims: AuthClaims,
) -> Result<AuthAccountId, ErrorStatus> {
    api.resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/registration-gate/webhook",
    description = "Webhook delivery status: whether the external Webhook is configured (`not_configured` means it is disabled, which is not an error: nothing is delivered and the gate is unaffected), a misconfiguration classification, the destination host only (never userinfo, path, query or the signing secret), delivery counts, the last success/failure times and the most recent delivery records. Delivery guarantees: at-least-once (the same event can arrive more than once after a retry, a crash or an admin retry, so receivers must de-duplicate by the stable event ID), no ordering guarantee, signed with HMAC-SHA256 over timestamp and body. The endpoint and secret are server configuration only and cannot be changed through the API. Delivery runs outside the gate state transition: Webhook failures never change the gate and never create new notification events.",
    responses(
        (status = 200, description = "Webhook status", body = WebhookStatusResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGateWebhook",
)]
pub(crate) async fn get_registration_gate_webhook_status(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateWebhookApi>,
) -> Result<Json<WebhookStatusResponse>, ErrorStatus> {
    let admin = resolve_admin(&api, claims).await?;
    let status = api
        .get_webhook_status(&admin)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json((&status).into()))
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/registration-gate/webhook/test",
    description = "Send one synthetic `webhook_test` event (event ID prefixed `test_`) to the configured endpoint through the same path as real deliveries (signature, destination check, timeout) and return the result. The test is a single attempt: it is not retried and no delivery record is stored. It never changes the gate state, the notification list or unread counts. Returns 409 when the Webhook is not configured or misconfigured (nothing is sent).",
    responses(
        (status = 200, description = "Test sent; `delivered` tells whether the receiver answered 2xx", body = WebhookTestResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 409, description = "Webhook is not configured or misconfigured"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGateWebhook",
)]
pub(crate) async fn send_registration_gate_webhook_test(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateWebhookApi>,
) -> Result<Json<WebhookTestResponse>, ErrorStatus> {
    let admin = resolve_admin(&api, claims).await?;
    let outcome = api
        .send_webhook_test(&admin)
        .await
        .map_err(ErrorStatus::from)?;
    match WebhookTestResponse::from_outcome(&outcome) {
        Some(response) => Ok(Json(response)),
        None => {
            let message = match outcome {
                WebhookTestOutcome::NotConfigured => "webhook is not configured",
                _ => "webhook configuration is invalid",
            };
            Err(ErrorStatus::from((
                StatusCode::CONFLICT,
                message.to_string(),
            )))
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/registration-gate/webhook/retry",
    description = "Requeue failed deliveries so they are sent again with the same event ID (at-least-once: the receiver may see the event more than once). With `event_id`, only that failed delivery is requeued (404 if it is not currently failed); without it, every failed delivery is requeued. Failed deliveries are never retried automatically.",
    request_body = WebhookRetryRequest,
    responses(
        (status = 200, description = "Number of requeued deliveries", body = WebhookRetryResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "No failed delivery with that event ID"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGateWebhook",
)]
pub(crate) async fn retry_registration_gate_webhooks(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateWebhookApi>,
    request: Result<Json<WebhookRetryRequest>, JsonRejection>,
) -> Result<Json<WebhookRetryResponse>, ErrorStatus> {
    let request = request
        .map(|Json(request)| request)
        .map_err(|rejection| bad_request(rejection.body_text()))?;
    let event_id = request
        .event_id
        .as_deref()
        .map(|raw| {
            raw.trim()
                .parse::<i64>()
                .map(RegistrationGateEventId::new)
                .map_err(|_| bad_request(format!("invalid event id: expected an integer: {raw}")))
        })
        .transpose()?;
    let admin = resolve_admin(&api, claims).await?;
    let requeued = api
        .retry_failed_webhooks(&admin, event_id)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(WebhookRetryResponse { requeued }))
}
