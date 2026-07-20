mod actor;
mod collections;
mod discovery;
mod inbox;

pub(crate) use actor::{__path_get_actor, get_actor};
pub(crate) use collections::{
    __path_get_followers, __path_get_following, __path_get_outbox, get_followers, get_following,
    get_outbox,
};
pub(crate) use discovery::{__path_webfinger, webfinger};
pub(crate) use inbox::{__path_post_inbox, post_inbox};

use crate::error::ErrorStatus;
use crate::handler::AppModule;
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use axum::extract::DefaultBodyLimit;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::prelude::entity::{Account, AccountId, Nanoid};
use serde::Serialize;

pub(super) const ACTIVITY_JSON: &str = "application/activity+json";
pub(super) const JRD_JSON: &str = "application/jrd+json";

pub trait ActivityPubRouter {
    fn route_activitypub(self) -> Self;
}

pub trait FederationRouter {
    fn route_federation(self) -> Self;
}

impl ActivityPubRouter for Router<AppModule> {
    fn route_activitypub(self) -> Self {
        self.route("/.well-known/webfinger", get(webfinger))
    }
}

impl FederationRouter for Router<AppModule> {
    fn route_federation(self) -> Self {
        self.route("/accounts/{account_id}", get(get_actor))
            .route(
                "/accounts/{account_id}/inbox",
                post(post_inbox).layer(DefaultBodyLimit::max(1024 * 1024)),
            )
            .route("/accounts/{account_id}/outbox", get(get_outbox))
            .route("/accounts/{account_id}/followers", get(get_followers))
            .route("/accounts/{account_id}/following", get(get_following))
    }
}

pub(super) async fn find_account_id_by_nanoid(
    module: &AppModule,
    id: String,
) -> Result<AccountId, ErrorStatus> {
    if id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let mut executor = module
        .database_connection()
        .get_executor()
        .await
        .map_err(ErrorStatus::from)?;
    let nanoid = Nanoid::<Account>::new(id);
    module
        .account_query_processor()
        .find_by_nanoid(&mut executor, &nanoid)
        .await
        .map_err(ErrorStatus::from)?
        .map(|account| account.id().clone())
        .ok_or_else(|| ErrorStatus::from(StatusCode::NOT_FOUND))
}

pub(super) fn json_response<T: Serialize>(
    value: &T,
    content_type: &'static str,
) -> Result<Response, ErrorStatus> {
    let body = serde_json::to_vec(value).map_err(|e| {
        ErrorStatus::from((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize JSON response: {e}"),
        ))
    })?;
    Ok(([(header::CONTENT_TYPE, content_type)], body).into_response())
}

pub(super) fn public_base_host_header(base_url: &str) -> Result<String, ErrorStatus> {
    let url = url::Url::parse(base_url).map_err(|e| {
        ErrorStatus::from((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Invalid PUBLIC_BASE_URL: {e}"),
        ))
    })?;
    let host = url.host_str().ok_or_else(|| {
        ErrorStatus::from((
            StatusCode::INTERNAL_SERVER_ERROR,
            "PUBLIC_BASE_URL must include a host".to_string(),
        ))
    })?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}
