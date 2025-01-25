use crate::error::ErrorStatus;
use crate::expect_role;
use crate::handler::AppModule;
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
    first: String,
    last: String,
    items: Vec<AccountResponse>,
}

pub trait AccountRouter {
    fn route_account(self, instance: KeycloakAuthInstance) -> Self;
}

impl AccountRouter for Router<AppModule> {
    fn route_account(self, instance: KeycloakAuthInstance) -> Self {
        self.route(
            "/accounts",
            get(
                |Extension(token): Extension<KeycloakToken<String>>,
                 State(module): State<AppModule>,
                 Query(GetAllAccountQuery {
                     direction,
                     limit,
                     cursor,
                 }): Query<GetAllAccountQuery>,
                 req: Request| async move {
                    expect_role!(&token, req);
                    let direction = direction.convert_to_direction()?;
                    let pagination = Pagination::new(limit, cursor, direction);
                    let result = module
                        .handler()
                        .pgpool()
                        .get_all_accounts(token.subject, pagination)
                        .await
                        .map_err(ErrorStatus::from)?;
                    if result.is_empty() {
                        return Err(ErrorStatus::from(StatusCode::NOT_FOUND));
                    }
                    let response = AccountsResponse {
                        first: result
                            .first()
                            .map(|account| account.nanoid.clone())
                            .unwrap(),
                        last: result.last().map(|account| account.nanoid.clone()).unwrap(),
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
                },
            ),
        )
        .layer(
            KeycloakAuthLayer::<String>::builder()
                .instance(instance)
                .passthrough_mode(PassthroughMode::Block)
                .expected_audiences(vec![String::from("account")])
                .build(),
        )
    }
}
