use super::{find_account_id_by_nanoid, json_response, ACTIVITY_JSON};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use application::service::activitypub::{GetFollowersCollectionUseCase, GetOutboxUseCase};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use std::collections::HashMap;

#[utoipa::path(
        get,
        path = "/ap/accounts/{account_id}/followers",
        description = "Retrieve an ActivityPub followers OrderedCollection for a local account.",
        params(("id" = String, Path, description = "Account nanoid")),
    responses(
        (status = 200, description = "Followers collection", body = kernel::activitypub::OrderedCollection, content_type = "application/activity+json"),
        (status = 400, description = "Invalid account ID"),
        (status = 404, description = "Account not found"),
    ),
    tag = "ActivityPub",
)]
pub(crate) async fn get_followers(
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
) -> Result<Response, ErrorStatus> {
    let account_id = find_account_id_by_nanoid(&module, account_id.clone())
        .await
        .map_err(|e| {
            tracing::debug!(nanoid = %account_id, error = ?e, "Followers collection not found");
            e
        })?;
    let collection = module
        .get_followers_collection(&account_id)
        .await
        .map_err(ErrorStatus::from)?;

    json_response(&collection, ACTIVITY_JSON)
}

#[utoipa::path(
        get,
        path = "/ap/accounts/{account_id}/following",
        description = "Retrieve an ActivityPub following OrderedCollection for a local account.",
        params(("id" = String, Path, description = "Account nanoid")),
    responses(
        (status = 200, description = "Following collection", body = kernel::activitypub::OrderedCollection, content_type = "application/activity+json"),
        (status = 400, description = "Invalid account ID"),
        (status = 404, description = "Account not found"),
    ),
    tag = "ActivityPub",
)]
pub(crate) async fn get_following(
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
) -> Result<Response, ErrorStatus> {
    let account_id = find_account_id_by_nanoid(&module, account_id.clone())
        .await
        .map_err(|e| {
            tracing::debug!(nanoid = %account_id, error = ?e, "Following collection not found");
            e
        })?;
    let collection = module
        .get_following_collection(&account_id)
        .await
        .map_err(ErrorStatus::from)?;

    json_response(&collection, ACTIVITY_JSON)
}

#[utoipa::path(
        get,
        path = "/ap/accounts/{account_id}/outbox",
        description = "Retrieve an ActivityPub outbox OrderedCollection for a local account.",
        params(
            ("id" = String, Path, description = "Account nanoid"),
        ("limit" = Option<usize>, Query, description = "Maximum number of activities to return"),
        ("cursor" = Option<i64>, Query, description = "Return activities with IDs older than this cursor")
    ),
    responses(
        (status = 200, description = "Outbox collection", body = kernel::activitypub::OrderedCollection, content_type = "application/activity+json"),
        (status = 400, description = "Invalid account ID or pagination parameter"),
        (status = 404, description = "Account not found"),
    ),
    tag = "ActivityPub",
)]
pub(crate) async fn get_outbox(
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ErrorStatus> {
    let account_id = find_account_id_by_nanoid(&module, account_id.clone())
        .await
        .map_err(|e| {
            tracing::debug!(nanoid = %account_id, error = ?e, "Outbox collection not found");
            e
        })?;
    let limit = parse_limit(params.get("limit"))?;
    let cursor = parse_cursor(params.get("cursor"))?;
    let collection = module
        .get_outbox_collection(&account_id, limit, cursor)
        .await
        .map_err(ErrorStatus::from)?;

    json_response(&collection, ACTIVITY_JSON)
}

fn parse_limit(limit: Option<&String>) -> Result<usize, ErrorStatus> {
    limit.map_or(Ok(20), |limit| {
        limit.parse::<usize>().map_err(|_| {
            ErrorStatus::from((
                StatusCode::BAD_REQUEST,
                "Invalid limit parameter".to_string(),
            ))
        })
    })
}

fn parse_cursor(cursor: Option<&String>) -> Result<Option<i64>, ErrorStatus> {
    cursor.map_or(Ok(None), |cursor| {
        cursor.parse::<i64>().map(Some).map_err(|_| {
            ErrorStatus::from((
                StatusCode::BAD_REQUEST,
                "Invalid cursor parameter".to_string(),
            ))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_limit_defaults_and_rejects_invalid_values() {
        assert_eq!(parse_limit(None).unwrap(), 20);
        assert_eq!(parse_limit(Some(&"5".to_string())).unwrap(), 5);
        assert!(parse_limit(Some(&"bad".to_string())).is_err());
    }

    #[test]
    fn parse_cursor_accepts_optional_i64() {
        assert_eq!(parse_cursor(None).unwrap(), None);
        assert_eq!(parse_cursor(Some(&"42".to_string())).unwrap(), Some(42));
        assert!(parse_cursor(Some(&"bad".to_string())).is_err());
    }
}
