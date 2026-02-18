use crate::error::ErrorStatus;
use crate::expect_role;
use crate::handler::AppModule;
use crate::keycloak::KeycloakAuthAccount;
use crate::route::DirectionConverter;
use application::service::account::{
    CreateAccountService, DeleteAccountService, EditAccountService, GetAccountService,
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
use std::ops::Deref;
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
    fn route_account(self, instance: KeycloakAuthInstance) -> Self;
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
    let direction = direction.convert_to_direction()?;
    let pagination = Pagination::new(limit, cursor, direction);
    let result = module
        .handler()
        .get_all_accounts(
            module.applier_container().deref(),
            auth_info.into(),
            pagination,
        )
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

    // サービス層でのアカウント作成処理の呼び出し
    let account = module
        .handler()
        .create_account(
            module.applier_container().deref(),
            auth_info.into(),
            request.name,
            request.is_bot,
        )
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

    // IDの検証
    if id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    // サービス層での特定アカウント取得処理
    let account = module
        .handler()
        .get_account_by_id(module.applier_container().deref(), auth_info.into(), id)
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

    // IDの検証
    if id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    // サービス層でのアカウント更新処理
    module
        .handler()
        .edit_account(
            module.applier_container().deref(),
            auth_info.into(),
            id,
            request.is_bot,
        )
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

    // IDの検証
    if id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    // サービス層でのアカウント削除処理
    module
        .handler()
        .delete_account(module.applier_container().deref(), auth_info.into(), id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

impl AccountRouter for Router<AppModule> {
    fn route_account(self, instance: KeycloakAuthInstance) -> Self {
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
