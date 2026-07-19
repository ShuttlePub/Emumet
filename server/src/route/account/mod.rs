mod admin;
mod client;
mod follow;
pub(crate) use admin::{
    __path_ban_account_by_id, __path_suspend_account_by_id, __path_unsuspend_account_by_id,
    ban_account_by_id, suspend_account_by_id, unsuspend_account_by_id,
};
pub(crate) use client::{
    __path_create_account, __path_deactivate_account_by_id, __path_get_account_by_id,
    __path_get_accounts, __path_update_account_by_id, create_account, deactivate_account_by_id,
    get_account_by_id, get_accounts, update_account_by_id,
};
pub(crate) use follow::{__path_follow_account, follow_account};

use crate::handler::AppModule;
use axum::routing::{delete, get, patch, post};
use axum::Router;

pub trait AccountRouter {
    fn route_account(self) -> Self;
}

pub trait AdminAccountRouter {
    fn route_admin_account(self) -> Self;
}

impl AccountRouter for Router<AppModule> {
    fn route_account(self) -> Self {
        self.route("/accounts", get(get_accounts))
            .route("/accounts", post(create_account))
            .route("/accounts/{account_id}", get(get_account_by_id))
            .route("/accounts/{account_id}", patch(update_account_by_id))
            .route("/accounts/{account_id}", delete(deactivate_account_by_id))
            .route("/accounts/{account_id}/follow", post(follow_account))
    }
}

impl AdminAccountRouter for Router<AppModule> {
    fn route_admin_account(self) -> Self {
        self.route(
            "/accounts/{account_id}/suspend",
            post(suspend_account_by_id),
        )
        .route(
            "/accounts/{account_id}/unsuspend",
            post(unsuspend_account_by_id),
        )
        .route("/accounts/{account_id}/ban", post(ban_account_by_id))
    }
}
