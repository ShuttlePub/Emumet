use crate::api::AdminRegistrationGateWebhookApi;
use crate::auth::{AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::schema::registration_gate_webhook::{
    parse_endpoint_id, CreateWebhookEndpointRequest, DeleteWebhookEndpointRequest,
    UpdateWebhookEndpointRequest, WebhookEndpointListResponse, WebhookEndpointResponse,
    WebhookFailedDeliveriesResponse, WebhookFailuresQuery, WebhookRetryRequest,
    WebhookRetryResponse, WebhookTestResponse,
};
use application::service::registration_gate_webhook::{
    CreateWebhookEndpointInput, UpdateWebhookEndpointInput,
};
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use error_stack::Report;
use kernel::interfaces::repository::WebhookTestOutcome;
use kernel::prelude::entity::{
    AuthAccountId, RegistrationGateEventId, WebhookEndpointId, WebhookEndpointRejection,
    WebhookFailureCursor,
};
use kernel::KernelError;

fn bad_request(message: impl Into<String>) -> ErrorStatus {
    ErrorStatus::from((StatusCode::BAD_REQUEST, message.into()))
}

/// 本文の解釈エラーは値を echo しない固定文言にする (secret を含みうる本文を返さない)。
fn parse_json<T>(request: Result<Json<T>, JsonRejection>) -> Result<T, ErrorStatus> {
    request
        .map(|Json(request)| request)
        .map_err(|_| bad_request("invalid request body"))
}

fn parse_endpoint(raw: &str) -> Result<WebhookEndpointId, ErrorStatus> {
    parse_endpoint_id(raw).ok_or_else(|| bad_request("invalid endpoint id"))
}

/// 拒否理由を HTTP に対応付ける: URL・secret・カテゴリの不正は 400、状態との衝突 (URL 重複・上限・
/// read-only・暗号鍵なし) は 409。本文は固定コードだけ (URL・secret・解決先アドレスを含まない)。
fn map_error(report: Report<KernelError>) -> ErrorStatus {
    if let Some(rejection) = report.downcast_ref::<WebhookEndpointRejection>() {
        let status = match rejection {
            WebhookEndpointRejection::DuplicateUrl
            | WebhookEndpointRejection::EndpointLimitReached
            | WebhookEndpointRejection::EnvironmentEndpointReadOnly
            | WebhookEndpointRejection::EncryptionKeyNotConfigured => StatusCode::CONFLICT,
            WebhookEndpointRejection::InvalidUrl
            | WebhookEndpointRejection::UnsupportedScheme
            | WebhookEndpointRejection::CredentialsInUrl
            | WebhookEndpointRejection::FragmentInUrl
            | WebhookEndpointRejection::MissingHost
            | WebhookEndpointRejection::UrlTooLong
            | WebhookEndpointRejection::ForbiddenDestination
            | WebhookEndpointRejection::HostUnresolvable
            | WebhookEndpointRejection::SecretTooShort
            | WebhookEndpointRejection::SecretTooLong
            | WebhookEndpointRejection::SecretRequired
            | WebhookEndpointRejection::NoCategories
            | WebhookEndpointRejection::NothingToUpdate => StatusCode::BAD_REQUEST,
        };
        return ErrorStatus::from((status, rejection.code().to_string()));
    }
    ErrorStatus::from(report)
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
    path = "/api/v1/admin/registration-gate/webhook/endpoints",
    description = "List the registration gate Webhook destinations (admin only). The URL is returned only as a partially masked display form (`masked_url`, e.g. `hooks.example.net/••••/a8f2`: host and port, then a mask and — only for a path token of 12+ characters — its last 4 characters); the full URL, path, query, userinfo and the signing secret are never returned. Each destination has a status chip (`healthy` / `failing`), per-destination delivery counts and failed count, and the selected notification categories. `has_failed_deliveries` tells whether any destination has an unresolved failed delivery (for a tab marker). The environment-variable destination of the previous single-destination setup appears as the read-only entry `id = environment` while the encryption key is not configured (it keeps delivering as before and cannot be edited or deleted through the API); it is imported as an ordinary destination at startup once the key is configured. Delivery guarantees: at-least-once (receivers must de-duplicate by the stable event ID), no ordering guarantee, signed per destination with HMAC-SHA256 over timestamp and body. Delivery runs outside the gate state transition: Webhook failures never change the gate and never create new notification events.",
    responses(
        (status = 200, description = "Destinations and overall state", body = WebhookEndpointListResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGateWebhook",
)]
pub(crate) async fn list_registration_gate_webhook_endpoints(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateWebhookApi>,
) -> Result<Json<WebhookEndpointListResponse>, ErrorStatus> {
    let admin = resolve_admin(&api, claims).await?;
    let list = api.list_endpoints(&admin).await.map_err(map_error)?;
    Ok(Json((&list).into()))
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/registration-gate/webhook/endpoints",
    description = "Register a Webhook destination (admin only; up to 10 destinations). The URL must be HTTPS without userinfo or fragment (max 2048 characters); it is checked at registration (host normalization, numeric/IPv6-zone/trailing-dot forms, `localhost`-style names, literal IPs and every resolved address) and re-checked on every connection. The secret (16-256 bytes) is write-only: it is encrypted (AEAD, bound to the destination ID) before storage and never returned, logged or audited; when the server has no encryption key configured the request is rejected with 409 `encryption_key_not_configured` and nothing is stored. A normalized URL that already exists is rejected with 409 `duplicate_url`; exceeding the limit is 409 `endpoint_limit_reached`. A new destination only receives events that occur after it is created (no backfill). Rejection bodies are fixed codes (`invalid_url`, `unsupported_scheme`, `credentials_in_url`, `fragment_in_url`, `missing_host`, `url_too_long`, `forbidden_destination`, `host_unresolvable`, `secret_too_short`, `secret_too_long`, `no_categories`) that never echo the URL, the secret or the resolved addresses.",
    request_body = CreateWebhookEndpointRequest,
    responses(
        (status = 201, description = "Destination created", body = WebhookEndpointResponse),
        (status = 400, description = "Invalid request (fixed rejection code in the body)"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 409, description = "Duplicate URL, destination limit reached or encryption key not configured (fixed code in the body)"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGateWebhook",
)]
pub(crate) async fn create_registration_gate_webhook_endpoint(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateWebhookApi>,
    request: Result<Json<CreateWebhookEndpointRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<WebhookEndpointResponse>), ErrorStatus> {
    let request = parse_json(request)?;
    let admin = resolve_admin(&api, claims).await?;
    let view = api
        .create_endpoint(
            &admin,
            CreateWebhookEndpointInput {
                url: request.url.clone(),
                secret: request.secret,
                categories: request.categories.iter().map(|id| (*id).into()).collect(),
            },
        )
        .await
        .map_err(map_error)?;
    Ok((StatusCode::CREATED, Json((&view).into())))
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/registration-gate/webhook/endpoints/{endpoint_id}",
    description = "Get one Webhook destination (admin only). Same masked shape as the list; never the full URL or the secret.",
    params(("endpoint_id" = String, Path, description = "Destination ID (decimal) or `environment`")),
    responses(
        (status = 200, description = "Destination", body = WebhookEndpointResponse),
        (status = 400, description = "Invalid destination ID"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "No such destination"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGateWebhook",
)]
pub(crate) async fn get_registration_gate_webhook_endpoint(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateWebhookApi>,
    Path(endpoint_id): Path<String>,
) -> Result<Json<WebhookEndpointResponse>, ErrorStatus> {
    let id = parse_endpoint(&endpoint_id)?;
    let admin = resolve_admin(&api, claims).await?;
    let view = api.get_endpoint(&admin, id).await.map_err(map_error)?;
    Ok(Json((&view).into()))
}

#[utoipa::path(
    put,
    path = "/api/v1/admin/registration-gate/webhook/endpoints/{endpoint_id}",
    description = "Update a Webhook destination (admin only; optimistic lock). `revision` is required and must match (409 otherwise); every success increments it. Omitting `url` or `secret` means \"keep as is\" — because neither is ever returned, an edit screen opens those fields empty and sends them only when the admin types a replacement. Omitting `categories` keeps the selection; when given it must have at least one category, and the change only affects events that occur after the update (deliveries already queued keep their state). The environment-variable destination cannot be edited (409 `environment_endpoint_read_only`). A request with nothing to change is 400 `nothing_to_update`.",
    params(("endpoint_id" = String, Path, description = "Destination ID (decimal)")),
    request_body = UpdateWebhookEndpointRequest,
    responses(
        (status = 200, description = "Destination updated", body = WebhookEndpointResponse),
        (status = 400, description = "Invalid request (fixed rejection code in the body)"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "No such destination"),
        (status = 409, description = "Revision conflict, duplicate URL, read-only destination or encryption key not configured"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGateWebhook",
)]
pub(crate) async fn update_registration_gate_webhook_endpoint(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateWebhookApi>,
    Path(endpoint_id): Path<String>,
    request: Result<Json<UpdateWebhookEndpointRequest>, JsonRejection>,
) -> Result<Json<WebhookEndpointResponse>, ErrorStatus> {
    let id = parse_endpoint(&endpoint_id)?;
    let request = parse_json(request)?;
    let admin = resolve_admin(&api, claims).await?;
    let view = api
        .update_endpoint(
            &admin,
            id,
            UpdateWebhookEndpointInput {
                revision: request.revision,
                url: request.url,
                secret: request.secret,
                categories: request
                    .categories
                    .map(|categories| categories.into_iter().map(Into::into).collect()),
            },
        )
        .await
        .map_err(map_error)?;
    Ok(Json((&view).into()))
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/registration-gate/webhook/endpoints/{endpoint_id}/delete",
    description = "Delete a Webhook destination (admin only; optimistic lock: `revision` must match, 409 otherwise). The destination's unfinished deliveries and delivery history are removed, so nothing further is delivered to it. This is a POST action (not HTTP DELETE) so the revision travels in the request body like the other admin actions. The environment-variable destination cannot be deleted (409 `environment_endpoint_read_only`).",
    params(("endpoint_id" = String, Path, description = "Destination ID (decimal)")),
    request_body = DeleteWebhookEndpointRequest,
    responses(
        (status = 204, description = "Destination deleted"),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "No such destination"),
        (status = 409, description = "Revision conflict or read-only destination"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGateWebhook",
)]
pub(crate) async fn delete_registration_gate_webhook_endpoint(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateWebhookApi>,
    Path(endpoint_id): Path<String>,
    request: Result<Json<DeleteWebhookEndpointRequest>, JsonRejection>,
) -> Result<StatusCode, ErrorStatus> {
    let id = parse_endpoint(&endpoint_id)?;
    let request = parse_json(request)?;
    let admin = resolve_admin(&api, claims).await?;
    api.delete_endpoint(&admin, id, request.revision)
        .await
        .map_err(map_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/registration-gate/webhook/endpoints/{endpoint_id}/test",
    description = "Send one synthetic `webhook_test` event (event ID prefixed `test_`) to this destination through the same path as real deliveries (its own signature, the registration-time destination check, timeout) and return the result. The test is a single attempt: it is not retried and no delivery record is stored. It never changes the gate state, the notification list, unread counts or the destination's status chip. `webhook_test` is not one of the selectable categories. For the read-only `environment` destination, 409 when the environment Webhook is not configured or is misconfigured (nothing is sent). A destination whose secret cannot be decrypted reports `secret_undecryptable` (nothing is sent).",
    params(("endpoint_id" = String, Path, description = "Destination ID (decimal) or `environment`")),
    responses(
        (status = 200, description = "Test sent; `delivered` tells whether the receiver answered 2xx", body = WebhookTestResponse),
        (status = 400, description = "Invalid destination ID"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "No such destination"),
        (status = 409, description = "Environment Webhook is not configured or misconfigured"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGateWebhook",
)]
pub(crate) async fn send_registration_gate_webhook_test(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateWebhookApi>,
    Path(endpoint_id): Path<String>,
) -> Result<Json<WebhookTestResponse>, ErrorStatus> {
    let id = parse_endpoint(&endpoint_id)?;
    let admin = resolve_admin(&api, claims).await?;
    let outcome = api.send_test(&admin, id).await.map_err(map_error)?;
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
    description = "Requeue failed deliveries so they are sent again with the same event ID (at-least-once: the receiver may see the event more than once). With `endpoint_id` and `event_id`, exactly that (destination, event) delivery is requeued (404 if it is not currently failed); with only `endpoint_id`, every failed delivery of that destination; with neither, every failed delivery of every destination. `event_id` without `endpoint_id` is 400. Failed deliveries are never retried automatically. Any failed delivery — however old — can be retried one at a time: use `GET .../webhook/failures` to page to it.",
    request_body = WebhookRetryRequest,
    responses(
        (status = 200, description = "Number of requeued deliveries", body = WebhookRetryResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "No such destination or no failed delivery with that event ID"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGateWebhook",
)]
pub(crate) async fn retry_registration_gate_webhooks(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateWebhookApi>,
    request: Result<Json<WebhookRetryRequest>, JsonRejection>,
) -> Result<Json<WebhookRetryResponse>, ErrorStatus> {
    let request = parse_json(request)?;
    let endpoint_id = request
        .endpoint_id
        .as_deref()
        .map(parse_endpoint)
        .transpose()?;
    let event_id = request
        .event_id
        .as_deref()
        .map(|raw| {
            raw.trim()
                .parse::<i64>()
                .map(RegistrationGateEventId::new)
                .map_err(|_| bad_request("invalid event id: expected an integer"))
        })
        .transpose()?;
    let admin = resolve_admin(&api, claims).await?;
    let requeued = api
        .retry_failed(&admin, endpoint_id, event_id)
        .await
        .map_err(map_error)?;
    Ok(Json(WebhookRetryResponse { requeued }))
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/registration-gate/webhook/failures",
    description = "List failed deliveries with cursor paging (admin only). Ordered by the time the delivery failed, newest first (ties by destination ID and event ID, descending), so the order is stable. Follow `next_cursor` (pass it as `before`) to reach every failed delivery regardless of how many there are — including ones older than the most recent 50 — and retry each one with `POST .../webhook/retry`. Filter by `endpoint_id`. Successful deliveries are never listed. Items carry the destination ID, event ID, event type, category, failure time and a result classification; never the URL, the secret or the payload.",
    params(WebhookFailuresQuery),
    responses(
        (status = 200, description = "A page of failed deliveries", body = WebhookFailedDeliveriesResponse),
        (status = 400, description = "Invalid query"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGateWebhook",
)]
pub(crate) async fn list_registration_gate_webhook_failures(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateWebhookApi>,
    query: Result<Query<WebhookFailuresQuery>, QueryRejection>,
) -> Result<Json<WebhookFailedDeliveriesResponse>, ErrorStatus> {
    let Query(query) = query.map_err(|_| bad_request("invalid query"))?;
    let endpoint_id = query
        .endpoint_id
        .as_deref()
        .map(parse_endpoint)
        .transpose()?;
    let before = query
        .before
        .as_deref()
        .map(|raw| {
            WebhookFailureCursor::decode(raw.trim()).ok_or_else(|| bad_request("invalid cursor"))
        })
        .transpose()?;
    let admin = resolve_admin(&api, claims).await?;
    let page = api
        .list_failed(&admin, endpoint_id, before, query.limit)
        .await
        .map_err(map_error)?;
    Ok(Json((&page).into()))
}
