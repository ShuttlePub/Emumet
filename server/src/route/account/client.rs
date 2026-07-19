use crate::auth::{resolve_auth_account_id, AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::route::{parse_comma_ids, DirectionConverter};
use crate::schema::account::{
    account_dto_to_response, AccountResponse, AccountsResponse, CreateAccountRequest,
    GetAllAccountQuery, UpdateAccountRequest,
};
use application::service::account::{CreateAccountUseCase, DeactivateAccountUseCase};
use application::service::account_detail::{GetAccountDetailUseCase, UpdateAccountDetailUseCase};
use application::transfer::pagination::Pagination;
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
    State(module): State<AppModule>,
    Query(GetAllAccountQuery {
        ids,
        direction,
        limit,
        cursor,
    }): Query<GetAllAccountQuery>,
) -> Result<Json<AccountsResponse>, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);
    let auth_account_id = resolve_auth_account_id(&module, auth_info)
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
        module
            .get_account_details_by_ids(&auth_account_id, id_list)
            .await
            .map_err(ErrorStatus::from)?
    } else {
        let direction = direction.convert_to_direction()?;
        let pagination = Pagination::new(limit, cursor, direction);
        module
            .get_all_account_details(&auth_account_id, pagination)
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
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
) -> Result<Json<AccountResponse>, ErrorStatus> {
    let auth_account_id = resolve_auth_account_id(&module, OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    let account = module
        .get_account_detail(&auth_account_id, account_id)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(account_dto_to_response(account)))
}

#[utoipa::path(
    post,
    path = "/api/v1/accounts",
    description = "Create a new account with a generated signing key pair.",
    request_body = CreateAccountRequest,
    responses(
        (status = 201, description = "Account created", body = AccountResponse),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn create_account(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Json(request): Json<CreateAccountRequest>,
) -> Result<(StatusCode, Json<crate::schema::account::AccountResponse>), ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

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

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let account = module
        .create_account(auth_account_id.clone(), request.into_dto())
        .await
        .map_err(ErrorStatus::from)?;
    let account = module
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
    State(module): State<AppModule>,
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

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let dto = request
        .into_dto(account_id)
        .map_err(|message| ErrorStatus::from((StatusCode::BAD_REQUEST, message.to_string())))?;
    let account = module
        .update_account_detail(&auth_account_id, dto)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(account_dto_to_response(account)))
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
        .deactivate_account(&auth_account_id, account_id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}
