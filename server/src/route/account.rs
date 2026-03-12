use crate::auth::{resolve_auth_account_id, AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::route::DirectionConverter;
use application::service::account::{
    CreateAccountUseCase, DeactivateAccountUseCase, EditAccountUseCase, GetAccountUseCase,
};
use application::transfer::pagination::Pagination;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
struct GetAllAccountQuery {
    limit: Option<u32>,
    cursor: Option<String>,
    direction: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateAccountRequest {
    name: String,
    is_bot: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateAccountRequest {
    is_bot: bool,
}

#[derive(Debug, Serialize)]
struct AccountResponse {
    id: String,
    name: String,
    public_key: String,
    is_bot: bool,
    created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct AccountsResponse {
    first: Option<String>,
    last: Option<String>,
    items: Vec<AccountResponse>,
}

pub trait AccountRouter {
    fn route_account(self) -> Self;
}

async fn get_accounts(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Query(GetAllAccountQuery {
        direction,
        limit,
        cursor,
    }): Query<GetAllAccountQuery>,
) -> Result<Json<AccountsResponse>, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);
    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let direction = direction.convert_to_direction()?;
    let pagination = Pagination::new(limit, cursor, direction);
    let result = module
        .get_all_accounts(&auth_account_id, pagination)
        .await
        .map_err(ErrorStatus::from)?
        .ok_or(ErrorStatus::from(StatusCode::NOT_FOUND))?;
    if result.is_empty() {
        return Err(ErrorStatus::from(StatusCode::NOT_FOUND));
    }
    let response = AccountsResponse {
        first: result.first().map(|account| account.nanoid.clone()),
        last: result.last().map(|account| account.nanoid.clone()),
        items: result
            .into_iter()
            .map(|account| AccountResponse {
                id: account.nanoid,
                name: account.name,
                public_key: account.public_key,
                is_bot: account.is_bot,
                created_at: account.created_at,
            })
            .collect(),
    };
    Ok(Json(response))
}

async fn create_account(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Json(request): Json<CreateAccountRequest>,
) -> Result<Json<AccountResponse>, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if request.name.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account name cannot be empty".to_string(),
        )));
    }

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let account = module
        .create_account(auth_account_id, request.name, request.is_bot)
        .await
        .map_err(ErrorStatus::from)?;

    let response = AccountResponse {
        id: account.nanoid,
        name: account.name,
        public_key: account.public_key,
        is_bot: account.is_bot,
        created_at: account.created_at,
    };

    Ok(Json(response))
}

async fn get_account_by_id(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(id): Path<String>,
) -> Result<Json<AccountResponse>, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let account = module
        .get_account_by_id(&auth_account_id, id)
        .await
        .map_err(ErrorStatus::from)?;

    let response = AccountResponse {
        id: account.nanoid,
        name: account.name,
        public_key: account.public_key,
        is_bot: account.is_bot,
        created_at: account.created_at,
    };

    Ok(Json(response))
}

async fn update_account_by_id(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(id): Path<String>,
    Json(request): Json<UpdateAccountRequest>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    module
        .edit_account(&auth_account_id, id, request.is_bot)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn deactivate_account_by_id(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(id): Path<String>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    module
        .deactivate_account(&auth_account_id, id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

impl AccountRouter for Router<AppModule> {
    fn route_account(self) -> Self {
        self.route("/accounts", get(get_accounts))
            .route("/accounts", post(create_account))
            .route("/accounts/:id", get(get_account_by_id))
            .route("/accounts/:id", put(update_account_by_id))
            .route("/accounts/:id", delete(deactivate_account_by_id))
    }
}
