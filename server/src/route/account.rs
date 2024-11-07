use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::route::DirectionConverter;
use application::service::account::GetAccountService;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
struct GetAllAccountQuery {
    limit: Option<i32>,
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
    fn route_account(self) -> Self;
}

impl AccountRouter for Router<AppModule> {
    fn route_account(self) -> Self {
        self.route(
            "/accounts",
            get(
                |State(module): State<AppModule>,
                 Query(GetAllAccountQuery {
                     direction,
                     limit,
                     cursor,
                 }): Query<GetAllAccountQuery>| async move {
                    let direction = direction.convert_to_direction()?;
                    let result = module
                        .handler()
                        .pgpool()
                        .get_all_accounts(limit, cursor, direction)
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
    }
}
