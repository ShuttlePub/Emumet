use crate::error::ErrorStatus;
use crate::expect_role;
use crate::handler::AppModule;
use crate::keycloak::{resolve_auth_account_id, KeycloakAuthAccount};
use crate::route::DirectionConverter;
use application::service::account::{
    CreateAccountUseCase, DeleteAccountUseCase, EditAccountUseCase, GetAccountUseCase,
};
use application::transfer::pagination::Pagination;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::{Method, Uri};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use axum_keycloak_auth::decode::KeycloakToken;
use axum_keycloak_auth::instance::KeycloakAuthInstance;
use axum_keycloak_auth::layer::KeycloakAuthLayer;
use axum_keycloak_auth::PassthroughMode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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
    fn route_account(self, instance: Arc<KeycloakAuthInstance>) -> Self;
}

async fn get_accounts(
    Extension(token): Extension<KeycloakToken<String>>,
    State(module): State<AppModule>,
    method: Method,
    uri: Uri,
    Query(GetAllAccountQuery {
        direction,
        limit,
        cursor,
    }): Query<GetAllAccountQuery>,
) -> Result<Json<AccountsResponse>, ErrorStatus> {
    expect_role!(&token, uri, method);
    let auth_info = KeycloakAuthAccount::from(token);
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
    Extension(token): Extension<KeycloakToken<String>>,
    State(module): State<AppModule>,
    method: Method,
    uri: Uri,
    Json(request): Json<CreateAccountRequest>,
) -> Result<Json<AccountResponse>, ErrorStatus> {
    expect_role!(&token, uri, method);
    let auth_info = KeycloakAuthAccount::from(token);

    // バリデーション
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
    Extension(token): Extension<KeycloakToken<String>>,
    State(module): State<AppModule>,
    method: Method,
    uri: Uri,
    Path(id): Path<String>,
) -> Result<Json<AccountResponse>, ErrorStatus> {
    expect_role!(&token, uri, method);
    let auth_info = KeycloakAuthAccount::from(token);

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
    Extension(token): Extension<KeycloakToken<String>>,
    State(module): State<AppModule>,
    method: Method,
    uri: Uri,
    Path(id): Path<String>,
    Json(request): Json<UpdateAccountRequest>,
) -> Result<StatusCode, ErrorStatus> {
    expect_role!(&token, uri, method);
    let auth_info = KeycloakAuthAccount::from(token);

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

async fn delete_account_by_id(
    Extension(token): Extension<KeycloakToken<String>>,
    State(module): State<AppModule>,
    method: Method,
    uri: Uri,
    Path(id): Path<String>,
) -> Result<StatusCode, ErrorStatus> {
    expect_role!(&token, uri, method);
    let auth_info = KeycloakAuthAccount::from(token);

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
        .delete_account(&auth_account_id, id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

impl AccountRouter for Router<AppModule> {
    fn route_account(self, instance: Arc<KeycloakAuthInstance>) -> Self {
        self.route("/accounts", get(get_accounts))
            .route("/accounts", post(create_account))
            .route("/accounts/:id", get(get_account_by_id))
            .route("/accounts/:id", put(update_account_by_id))
            .route("/accounts/:id", delete(delete_account_by_id))
            .layer(
                KeycloakAuthLayer::<String>::builder()
                    .instance(instance)
                    .passthrough_mode(PassthroughMode::Block)
                    .expected_audiences(vec![String::from("account")])
                    .build(),
            )
    }
}
