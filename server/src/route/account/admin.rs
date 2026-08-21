use crate::api::AdminAccountApi;
use crate::auth::{AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::schema::account::{BanAccountRequest, SuspendAccountRequest};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use kernel::interfaces::permission::InstanceRole;

fn parse_instance_role(role: &str) -> Result<InstanceRole, ErrorStatus> {
    match role {
        "admin" => Ok(InstanceRole::Admin),
        "moderator" => Ok(InstanceRole::Moderator),
        _ => Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            format!("invalid instance role: expected \"admin\" or \"moderator\": {role}"),
        ))),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/accounts/{account_id}/suspend",
    description = "Suspend an account with a reason and optional expiry.",
    params(("account_id" = String, Path, description = "Account nanoid")),
    request_body = SuspendAccountRequest,
    responses(
        (status = 204, description = "Account suspended"),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn suspend_account_by_id(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminAccountApi>,
    Path(account_id): Path<String>,
    Json(request): Json<SuspendAccountRequest>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let auth_account_id = api
        .resolve_auth_account_id(auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    api.suspend_account(
        &auth_account_id,
        account_id,
        request.reason,
        request.expires_at,
    )
    .await
    .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/accounts/{account_id}/unsuspend",
    description = "Remove suspension from an account.",
    params(("account_id" = String, Path, description = "Account nanoid")),
    responses(
        (status = 204, description = "Account unsuspended"),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn unsuspend_account_by_id(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminAccountApi>,
    Path(account_id): Path<String>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let auth_account_id = api
        .resolve_auth_account_id(auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    api.unsuspend_account(&auth_account_id, account_id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/accounts/{account_id}/ban",
    description = "Permanently ban an account.",
    params(("account_id" = String, Path, description = "Account nanoid")),
    request_body = BanAccountRequest,
    responses(
        (status = 204, description = "Account banned"),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn ban_account_by_id(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminAccountApi>,
    Path(account_id): Path<String>,
    Json(request): Json<BanAccountRequest>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let auth_account_id = api
        .resolve_auth_account_id(auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    api.ban_account(&auth_account_id, account_id, request.reason)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/accounts/{account_id}/unban",
    description = "Remove ban from an account.",
    params(("account_id" = String, Path, description = "Account nanoid")),
    responses(
        (status = 204, description = "Account unbanned"),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "Account not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn unban_account_by_id(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminAccountApi>,
    Path(account_id): Path<String>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let auth_account_id = api
        .resolve_auth_account_id(auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    api.unban_account(&auth_account_id, account_id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put,
    path = "/api/v1/admin/accounts/{account_id}/roles/{role}",
    description = "Assign an instance role (admin or moderator) to the owner of an account.",
    params(
        ("account_id" = String, Path, description = "Account nanoid"),
        ("role" = String, Path, description = "Instance role: admin or moderator"),
    ),
    responses(
        (status = 204, description = "Instance role assigned"),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "Account not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn assign_instance_role(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminAccountApi>,
    Path((account_id, role)): Path<(String, String)>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let role = parse_instance_role(&role)?;

    let auth_account_id = api
        .resolve_auth_account_id(auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    api.assign_instance_role(&auth_account_id, account_id, role)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/v1/admin/accounts/{account_id}/roles/{role}",
    description = "Revoke an instance role (admin or moderator) from the owner of an account. Admins cannot revoke their own admin role.",
    params(
        ("account_id" = String, Path, description = "Account nanoid"),
        ("role" = String, Path, description = "Instance role: admin or moderator"),
    ),
    responses(
        (status = 204, description = "Instance role revoked"),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "Account not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn revoke_instance_role(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminAccountApi>,
    Path((account_id, role)): Path<(String, String)>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let role = parse_instance_role(&role)?;

    let auth_account_id = api
        .resolve_auth_account_id(auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    api.revoke_instance_role(&auth_account_id, account_id, role)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}
