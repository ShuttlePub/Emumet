use crate::error::ErrorStatus;
use crate::expect_role;
use crate::handler::AppModule;
use crate::keycloak::KeycloakAuthAccount;
use crate::route::DirectionConverter;
use application::service::account::GetAccountService;
use application::transfer::pagination::Pagination;
use axum::extract::{Query, Request, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router};
use axum_keycloak_auth::decode::KeycloakToken;
use axum_keycloak_auth::instance::KeycloakAuthInstance;
use axum_keycloak_auth::layer::KeycloakAuthLayer;
use axum_keycloak_auth::PassthroughMode;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
struct GetAllAccountQuery {
    limit: Option<u32>,
    cursor: Option<String>,
    direction: Option<String>,
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
    Query(GetAllAccountQuery {
        direction,
        limit,
        cursor,
    }): Query<GetAllAccountQuery>,
    req: Request,
) -> Result<Json<AccountsResponse>, ErrorStatus> {
    expect_role!(&token, req);
    let auth_info = KeycloakAuthAccount::from(token);
    let direction = direction.convert_to_direction()?;
    let pagination = Pagination::new(limit, cursor, direction);
    let result = module
        .handler()
        .pgpool()
        .get_all_accounts(auth_info.into(), pagination)
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

impl AccountRouter for Router<AppModule> {
    fn route_account(self, instance: KeycloakAuthInstance) -> Self {
        self.route("/accounts", get(get_accounts))
            // .route("/accounts", post(todo!()))
            // .route("/accounts/:id", get(todo!()))
            // .route("/accounts/:id", delete(todo!()))
            .layer(
                KeycloakAuthLayer::<String>::builder()
                    .instance(instance)
                    .passthrough_mode(PassthroughMode::Block)
                    .expected_audiences(vec![String::from("account")])
                    .build(),
            )
    }
}
