use crate::api::AccountApi;
use crate::auth::{AuthClaims, OidcAuthInfo, RequestOrganizationContext, ORGANIZATION_ID_HEADER};
use crate::error::ErrorStatus;
use crate::route::{parse_comma_ids, DirectionConverter};
use crate::schema::account::{
    account_dto_to_response, AccountResponse, AccountsResponse, CreateAccountRequest,
    CreateProfileRequest, GetAllAccountQuery, ProfileResponse, SetProfileUsernameRequest,
    UpdateAccountRequest,
};
use crate::schema::profile_username::validate_optional_username;
use application::dto::pagination::Pagination;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

#[utoipa::path(
    get,
    path = "/api/v1/accounts",
    description = "Retrieve accounts by IDs or with cursor-based pagination.",
    params(
        ("ids" = Option<String>, Query, description = "Comma-separated account IDs"),
        ("limit" = Option<u32>, Query, description = "Pagination limit"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
        ("direction" = Option<String>, Query, description = "Pagination direction (asc/desc)"),
    ),
    responses(
        (status = 200, description = "List of accounts", body = AccountsResponse),
        (status = 400, description = "Invalid request parameters"),
        (status = 404, description = "No accounts found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn get_accounts(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AccountApi>,
    Query(GetAllAccountQuery {
        ids,
        direction,
        limit,
        cursor,
    }): Query<GetAllAccountQuery>,
) -> Result<Json<AccountsResponse>, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);
    let auth_account_id = api
        .resolve_auth_account_id(auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let result = if let Some(ids) = ids {
        if limit.is_some() || cursor.is_some() || direction.is_some() {
            return Err(ErrorStatus::from((
                StatusCode::BAD_REQUEST,
                "Cannot use ids with pagination parameters".to_string(),
            )));
        }
        let id_list = parse_comma_ids(&ids)?;
        api.get_account_details_by_ids(&auth_account_id, id_list)
            .await
            .map_err(ErrorStatus::from)?
    } else {
        let direction = direction.convert_to_direction()?;
        let pagination = Pagination::new(limit, cursor, direction);
        api.get_all_account_details(&auth_account_id, pagination)
            .await
            .map_err(ErrorStatus::from)?
    };

    let response = AccountsResponse {
        first: result.first().map(|account| account.nanoid.clone()),
        last: result.last().map(|account| account.nanoid.clone()),
        items: result.into_iter().map(account_dto_to_response).collect(),
    };
    Ok(Json(response))
}

#[utoipa::path(
    get,
    path = "/api/v1/accounts/{account_id}",
    description = "Retrieve one integrated account resource.",
    params(("account_id" = String, Path, description = "Account nanoid")),
    responses(
        (status = 200, description = "Integrated account", body = AccountResponse),
        (status = 404, description = "Account not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn get_account_by_id(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AccountApi>,
    Path(account_id): Path<String>,
) -> Result<Json<AccountResponse>, ErrorStatus> {
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    let account = api
        .get_account_detail(&auth_account_id, account_id)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(account_dto_to_response(account)))
}

#[utoipa::path(
    post,
    path = "/api/v1/accounts",
    description = "Create a new account with a generated signing key pair. `profile_username` is optional: a valid value is stored on the created Profile (unique across all Profiles including deleted ones, immutable afterwards); omit it to keep the legacy behaviour.",
    request_body = CreateAccountRequest,
    responses(
        (status = 201, description = "Account created", body = AccountResponse),
        (status = 400, description = "Invalid request (including a malformed or reserved profile_username)"),
        (status = 409, description = "profile_username is already taken; nothing was created"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn create_account(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AccountApi>,
    Json(request): Json<CreateAccountRequest>,
) -> Result<(StatusCode, Json<crate::schema::account::AccountResponse>), ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    validate_optional_username(request.profile_username.as_deref())?;

    if request.name.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account name cannot be empty".to_string(),
        )));
    }

    if request.name.len() > 100 {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account name must not exceed 100 characters".to_string(),
        )));
    }

    let auth_account_id = api
        .resolve_auth_account_id(auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let profile_username = request.profile_username.clone();
    let account = api
        .create_account_with_profile_username(
            auth_account_id.clone(),
            request.into_dto(),
            profile_username,
        )
        .await
        .map_err(ErrorStatus::from)?;
    let account = api
        .get_account_detail(&auth_account_id, account.nanoid)
        .await
        .map_err(ErrorStatus::from)?;
    Ok((StatusCode::CREATED, Json(account_dto_to_response(account))))
}

#[utoipa::path(
    patch,
    path = "/api/v1/accounts/{account_id}",
    description = "Update account properties.",
    params(("account_id" = String, Path, description = "Account nanoid")),
    request_body = UpdateAccountRequest,
    responses(
        (status = 200, description = "Account updated", body = AccountResponse),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn update_account_by_id(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<AccountApi>,
    Path(account_id): Path<String>,
    Json(request): Json<UpdateAccountRequest>,
) -> Result<Json<AccountResponse>, ErrorStatus> {
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

    let dto = request
        .into_dto(account_id)
        .map_err(|message| ErrorStatus::from((StatusCode::BAD_REQUEST, message.to_string())))?;
    let account = api
        .update_account_detail_in_context(&auth_account_id, org_context.as_ref(), dto)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(account_dto_to_response(account)))
}

#[utoipa::path(
    post,
    path = "/api/v1/profiles",
    description = "Create an organization-owned Profile. `username` is optional: a valid value is stored on the created Profile (unique across all Profiles including deleted ones, immutable afterwards).",
    params(("X-Organization-Id" = String, Header, description = "Organization account nanoid that owns the profile")),
    request_body = CreateProfileRequest,
    responses(
        (status = 201, body = ProfileResponse),
        (status = 400, description = "Missing organization header or malformed/reserved username"),
        (status = 403, description = "Authenticated person is not an active organization member"),
        (status = 404, description = "Organization not found"),
        (status = 409, description = "username is already taken; nothing was created"),
        (status = 422, description = "Organization profile already exists"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn create_profile(
    Extension(_claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<AccountApi>,
    Json(request): Json<CreateProfileRequest>,
) -> Result<(StatusCode, Json<ProfileResponse>), ErrorStatus> {
    let org_context = org_context.ok_or_else(|| {
        ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            format!("{ORGANIZATION_ID_HEADER} header is required"),
        ))
    })?;
    validate_optional_username(request.username.as_deref())?;
    let profile = api
        .create_organization_profile(org_context, request.into_dto())
        .await
        .map_err(ErrorStatus::from)?;
    Ok((StatusCode::CREATED, Json(profile.into())))
}

#[utoipa::path(
    post,
    path = "/api/v1/profiles/{profile_nanoid}/username",
    description = "Set the username of a Profile that has none, exactly once. This is the only way to set a username after creation: there is no update API, a Profile that already has a username is refused with 409, and the username is unique across all Profiles (also deleted ones). Authorization: only the Profile's current owner (the owning personal Account, or an active member of the owning organization via X-Organization-Id). The username becomes the acct name of the Profile's future ActivityPub actor; the actor itself is not published before the P4 cutover.",
    params(
        ("profile_nanoid" = String, Path, description = "Profile nanoid"),
        ("X-Organization-Id" = Option<String>, Header, description = "Organization account nanoid when the Profile is organization-owned"),
    ),
    request_body = SetProfileUsernameRequest,
    responses(
        (status = 200, description = "Username set", body = ProfileResponse),
        (status = 400, description = "Malformed or reserved username"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
        (status = 409, description = "The Profile already has a username, or the username is already taken"),
        (status = 422, description = "The owning Account is suspended or banned"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn set_profile_username(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<AccountApi>,
    Path(profile_nanoid): Path<String>,
    Json(request): Json<SetProfileUsernameRequest>,
) -> Result<Json<ProfileResponse>, ErrorStatus> {
    validate_optional_username(Some(&request.username))?;
    if profile_nanoid.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Profile ID cannot be empty".to_string(),
        )));
    }
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    let profile = api
        .set_profile_username(
            &auth_account_id,
            org_context.as_ref(),
            profile_nanoid,
            request.username,
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(profile.into()))
}

#[utoipa::path(
    delete,
    path = "/api/v1/accounts/{account_id}",
    description = "Deactivate an account and cascade-delete related resources.",
    params(("account_id" = String, Path, description = "Account nanoid")),
    responses(
        (status = 204, description = "Account deactivated"),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn deactivate_account_by_id(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AccountApi>,
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

    api.deactivate_account(&auth_account_id, account_id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/accounts/{account_id}/reactivate",
    description = "Reactivate a deactivated account owned by the authenticated principal.",
    params(("account_id" = String, Path, description = "Account nanoid")),
    responses(
        (status = 204, description = "Account reactivated"),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Account is not linked to the authenticated principal"),
        (status = 404, description = "Account not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn reactivate_account_by_id(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AccountApi>,
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

    api.reactivate_account(&auth_account_id, account_id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
