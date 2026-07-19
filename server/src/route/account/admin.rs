use crate::auth::{resolve_auth_account_id, AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::schema::account::{BanAccountRequest, SuspendAccountRequest};
use application::service::account::{
    BanAccountUseCase, SuspendAccountUseCase, UnsuspendAccountUseCase,
};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

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
    State(module): State<AppModule>,
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

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    module
        .suspend_account(
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
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    module
        .unsuspend_account(&auth_account_id, account_id)
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
    State(module): State<AppModule>,
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

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    module
        .ban_account(&auth_account_id, account_id, request.reason)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}
