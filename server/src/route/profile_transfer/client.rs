use crate::api::ProfileTransferApi;
use crate::auth::{AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::schema::profile_transfer::{
    CreateProfileTransferRequest, ProfileTransferRequestResponse,
};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

#[utoipa::path(
    post,
    path = "/api/v1/profiles/{profile_nanoid}/transfer-requests",
    description = "Request transfer of a personal profile to an organization.",
    request_body = CreateProfileTransferRequest,
    responses(
        (status = 201, description = "Transfer request created", body = ProfileTransferRequestResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "Profile or organization not found"),
        (status = 422, description = "Duplicate pending request"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfileTransfer",
)]
pub(crate) async fn create_profile_transfer_request(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<ProfileTransferApi>,
    Path(profile_nanoid): Path<String>,
    Json(request): Json<CreateProfileTransferRequest>,
) -> Result<(StatusCode, Json<ProfileTransferRequestResponse>), ErrorStatus> {
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    let response = api
        .request_profile_transfer(&auth_account_id, profile_nanoid, request.org_account_nanoid)
        .await
        .map_err(ErrorStatus::from)?;
    Ok((StatusCode::CREATED, Json((&response).into())))
}

#[utoipa::path(
    post,
    path = "/api/v1/profile-transfer-requests/{request_nanoid}/accept",
    description = "Accept a profile transfer request as an organization owner or admin.",
    responses(
        (status = 204, description = "Transfer accepted"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "Transfer request not found"),
        (status = 422, description = "Request not pending"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfileTransfer",
)]
pub(crate) async fn accept_profile_transfer_request(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<ProfileTransferApi>,
    Path(request_nanoid): Path<String>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    api.accept_profile_transfer_request(&auth_account_id, request_nanoid)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/profile-transfer-requests/{request_nanoid}/reject",
    description = "Reject a profile transfer request as an organization owner or admin.",
    responses(
        (status = 204, description = "Transfer rejected"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "Transfer request not found"),
        (status = 422, description = "Request not pending"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfileTransfer",
)]
pub(crate) async fn reject_profile_transfer_request(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<ProfileTransferApi>,
    Path(request_nanoid): Path<String>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    api.reject_profile_transfer_request(&auth_account_id, request_nanoid)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/profile-transfer-requests/{request_nanoid}/cancel",
    description = "Cancel a pending profile transfer request as the requester.",
    responses(
        (status = 204, description = "Transfer request cancelled"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "Transfer request not found"),
        (status = 422, description = "Request not pending"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfileTransfer",
)]
pub(crate) async fn cancel_profile_transfer_request(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<ProfileTransferApi>,
    Path(request_nanoid): Path<String>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    api.cancel_profile_transfer_request(&auth_account_id, request_nanoid)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(StatusCode::NO_CONTENT)
}
