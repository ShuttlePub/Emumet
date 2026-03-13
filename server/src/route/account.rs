use crate::auth::{resolve_auth_account_id, AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::route::{parse_comma_ids, DirectionConverter};
use crate::schema::account::{
    account_dto_to_response, AccountsResponse, BanAccountRequest, CreateAccountRequest,
    SuspendAccountRequest, UpdateAccountRequest,
};
use application::service::account::{
    BanAccountUseCase, CreateAccountUseCase, DeactivateAccountUseCase, EditAccountUseCase,
    GetAccountUseCase, SuspendAccountUseCase, UnsuspendAccountUseCase,
};
use application::transfer::pagination::Pagination;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};

use crate::schema::account::GetAllAccountQuery;

pub trait AccountRouter {
    fn route_account(self) -> Self;
}

async fn get_accounts(
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
            .get_accounts_by_ids(&auth_account_id, id_list)
            .await
            .map_err(ErrorStatus::from)?
    } else {
        let direction = direction.convert_to_direction()?;
        let pagination = Pagination::new(limit, cursor, direction);
        module
            .get_all_accounts(&auth_account_id, pagination)
            .await
            .map_err(ErrorStatus::from)?
            .ok_or(ErrorStatus::from(StatusCode::NOT_FOUND))?
    };

    if result.is_empty() {
        return Err(ErrorStatus::from(StatusCode::NOT_FOUND));
    }
    let response = AccountsResponse {
        first: result.first().map(|account| account.nanoid.clone()),
        last: result.last().map(|account| account.nanoid.clone()),
        items: result.into_iter().map(account_dto_to_response).collect(),
    };
    Ok(Json(response))
}

async fn create_account(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Json(request): Json<CreateAccountRequest>,
) -> Result<Json<crate::schema::account::AccountResponse>, ErrorStatus> {
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

    Ok(Json(account_dto_to_response(account)))
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

async fn suspend_account_by_id(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(id): Path<String>,
    Json(request): Json<SuspendAccountRequest>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    if request.reason.trim().is_empty() || request.reason.len() > 1000 {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Reason must be between 1 and 1000 characters".to_string(),
        )));
    }

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    module
        .suspend_account(&auth_account_id, id, request.reason, request.expires_at)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn unsuspend_account_by_id(
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
        .unsuspend_account(&auth_account_id, id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn ban_account_by_id(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(id): Path<String>,
    Json(request): Json<BanAccountRequest>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    if request.reason.trim().is_empty() || request.reason.len() > 1000 {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Reason must be between 1 and 1000 characters".to_string(),
        )));
    }

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    module
        .ban_account(&auth_account_id, id, request.reason)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

impl AccountRouter for Router<AppModule> {
    fn route_account(self) -> Self {
        self.route("/accounts", get(get_accounts))
            .route("/accounts", post(create_account))
            .route("/accounts/:id", put(update_account_by_id))
            .route("/accounts/:id", delete(deactivate_account_by_id))
            .route("/accounts/:id/suspend", post(suspend_account_by_id))
            .route("/accounts/:id/unsuspend", post(unsuspend_account_by_id))
            .route("/accounts/:id/ban", post(ban_account_by_id))
    }
}
