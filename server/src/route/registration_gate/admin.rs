use crate::api::AdminRegistrationGateApi;
use crate::auth::{AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::schema::registration_gate::{
    EnableOverrideRequest, MarkNotificationsReadRequest, NotificationsQuery,
    RegistrationGateNotificationListResponse, RegistrationGateResponse,
    RegistrationGateRevisionRequest, UnreadCountResponse,
};
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use kernel::prelude::entity::{AuthAccountId, RegistrationGateEventId};

fn bad_request(message: impl Into<String>) -> ErrorStatus {
    ErrorStatus::from((StatusCode::BAD_REQUEST, message.into()))
}

fn parse_json<T>(request: Result<Json<T>, JsonRejection>) -> Result<T, ErrorStatus> {
    request
        .map(|Json(request)| request)
        .map_err(|rejection| bad_request(rejection.body_text()))
}

fn parse_event_id(raw: &str) -> Result<RegistrationGateEventId, ErrorStatus> {
    raw.trim()
        .parse::<i64>()
        .map(RegistrationGateEventId::new)
        .map_err(|_| bad_request(format!("invalid event id: expected an integer: {raw}")))
}

async fn resolve_admin(
    api: &AdminRegistrationGateApi,
    claims: AuthClaims,
) -> Result<AuthAccountId, ErrorStatus> {
    api.resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/registration-gate",
    description = "Get the registration gate state. `override.active` is true while the override is in effect so the admin UI can keep showing the warning on every page. The authenticated Kratos pre-persist hook enforces this decision on self-service registration. Enforcement attestation loss stops registration ahead of override and requires manual resume after restoration.",
    responses(
        (status = 200, description = "Registration gate state", body = RegistrationGateResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGate",
)]
pub(crate) async fn get_registration_gate(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateApi>,
) -> Result<Json<RegistrationGateResponse>, ErrorStatus> {
    let admin = resolve_admin(&api, claims).await?;
    let view = api
        .get_registration_gate(&admin)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json((&view).into()))
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/registration-gate/open",
    description = "Open (or manually resume after a config-loss stop) the registration gate. Succeeds only when at least one protection (email or bot) is verified; clears the stop latch. The authenticated Kratos pre-persist hook enforces this decision on self-service registration. Enforcement attestation loss stops registration ahead of override and requires manual resume after restoration.",
    request_body = RegistrationGateRevisionRequest,
    responses(
        (status = 200, description = "Gate opened", body = RegistrationGateResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 409, description = "Revision conflict"),
        (status = 422, description = "Minimum protection condition not met, or already open"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGate",
)]
pub(crate) async fn open_registration_gate(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateApi>,
    request: Result<Json<RegistrationGateRevisionRequest>, JsonRejection>,
) -> Result<Json<RegistrationGateResponse>, ErrorStatus> {
    let request = parse_json(request)?;
    let admin = resolve_admin(&api, claims).await?;
    let view = api
        .open_registration_gate(&admin, request.revision)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json((&view).into()))
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/registration-gate/close",
    description = "Manually close the registration gate. The closure is never lifted automatically. The authenticated Kratos pre-persist hook enforces this decision on self-service registration. Enforcement attestation loss stops registration ahead of override and requires manual resume after restoration.",
    request_body = RegistrationGateRevisionRequest,
    responses(
        (status = 200, description = "Gate closed", body = RegistrationGateResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 409, description = "Revision conflict"),
        (status = 422, description = "Already closed"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGate",
)]
pub(crate) async fn close_registration_gate(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateApi>,
    request: Result<Json<RegistrationGateRevisionRequest>, JsonRejection>,
) -> Result<Json<RegistrationGateResponse>, ErrorStatus> {
    let request = parse_json(request)?;
    let admin = resolve_admin(&api, claims).await?;
    let view = api
        .close_registration_gate(&admin, request.revision)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json((&view).into()))
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/registration-gate/override",
    description = "Open the registration gate with override: allowed even without a verified protection, but only after the warning is explicitly acknowledged. Stays in effect until explicitly revoked; automatic stop and outage rejection are not applied while it is active (verified protections are still enforced). The authenticated Kratos pre-persist hook enforces this decision on self-service registration. Enforcement attestation loss stops registration ahead of override and requires manual resume after restoration.",
    request_body = EnableOverrideRequest,
    responses(
        (status = 200, description = "Override enabled and gate opened", body = RegistrationGateResponse),
        (status = 400, description = "Invalid request or warning not acknowledged"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 409, description = "Revision conflict"),
        (status = 422, description = "Override already active"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGate",
)]
pub(crate) async fn enable_registration_gate_override(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateApi>,
    request: Result<Json<EnableOverrideRequest>, JsonRejection>,
) -> Result<Json<RegistrationGateResponse>, ErrorStatus> {
    let request = parse_json(request)?;
    let admin = resolve_admin(&api, claims).await?;
    let view = api
        .enable_registration_gate_override(&admin, request.revision, request.acknowledge_warning)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json((&view).into()))
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/registration-gate/override/revoke",
    description = "Revoke the override and return to normal judgement, re-evaluated against the current observation. Never closes the gate by itself. The authenticated Kratos pre-persist hook enforces this decision on self-service registration. Enforcement attestation loss stops registration ahead of override and requires manual resume after restoration.",
    request_body = RegistrationGateRevisionRequest,
    responses(
        (status = 200, description = "Override revoked", body = RegistrationGateResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 409, description = "Revision conflict"),
        (status = 422, description = "Override is not active"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGate",
)]
pub(crate) async fn revoke_registration_gate_override(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateApi>,
    request: Result<Json<RegistrationGateRevisionRequest>, JsonRejection>,
) -> Result<Json<RegistrationGateResponse>, ErrorStatus> {
    let request = parse_json(request)?;
    let admin = resolve_admin(&api, claims).await?;
    let view = api
        .revoke_registration_gate_override(&admin, request.revision)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json((&view).into()))
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/registration-gate/notifications",
    description = "List registration gate notification events (newest first) with the caller's own read state.",
    params(NotificationsQuery),
    responses(
        (status = 200, description = "Notification events", body = RegistrationGateNotificationListResponse),
        (status = 400, description = "Invalid query"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGate",
)]
pub(crate) async fn list_registration_gate_notifications(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateApi>,
    query: Result<Query<NotificationsQuery>, QueryRejection>,
) -> Result<Json<RegistrationGateNotificationListResponse>, ErrorStatus> {
    let Query(query) = query.map_err(|rejection| bad_request(rejection.body_text()))?;
    let before = query.before.as_deref().map(parse_event_id).transpose()?;
    let admin = resolve_admin(&api, claims).await?;
    let notifications = api
        .list_notifications(&admin, before, query.limit)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json((&notifications).into()))
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/registration-gate/notifications/unread-count",
    description = "Unread notification count for the calling admin only.",
    responses(
        (status = 200, description = "Unread count", body = UnreadCountResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGate",
)]
pub(crate) async fn unread_registration_gate_notifications(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateApi>,
) -> Result<Json<UnreadCountResponse>, ErrorStatus> {
    let admin = resolve_admin(&api, claims).await?;
    let unread_count = api
        .count_unread_notifications(&admin)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(UnreadCountResponse { unread_count }))
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/registration-gate/notifications/read",
    description = "Mark the given notification events as read for the calling admin only. Other admins' read state is unaffected.",
    request_body = MarkNotificationsReadRequest,
    responses(
        (status = 200, description = "Unread count after marking", body = UnreadCountResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGate",
)]
pub(crate) async fn mark_registration_gate_notifications_read(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateApi>,
    request: Result<Json<MarkNotificationsReadRequest>, JsonRejection>,
) -> Result<Json<UnreadCountResponse>, ErrorStatus> {
    let request = parse_json(request)?;
    let event_ids = request
        .event_ids
        .iter()
        .map(|raw| parse_event_id(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let admin = resolve_admin(&api, claims).await?;
    let unread_count = api
        .mark_notifications_read(&admin, event_ids)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(UnreadCountResponse { unread_count }))
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/registration-gate/notifications/read-all",
    description = "Mark every notification event as read for the calling admin only.",
    responses(
        (status = 200, description = "Unread count after marking", body = UnreadCountResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
    ),
    security(("bearer_auth" = [])),
    tag = "RegistrationGate",
)]
pub(crate) async fn mark_all_registration_gate_notifications_read(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminRegistrationGateApi>,
) -> Result<Json<UnreadCountResponse>, ErrorStatus> {
    let admin = resolve_admin(&api, claims).await?;
    let unread_count = api
        .mark_all_notifications_read(&admin)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(UnreadCountResponse { unread_count }))
}
