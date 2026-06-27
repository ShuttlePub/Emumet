use crate::error::ErrorStatus;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::collections::HashMap;

async fn get_pool() -> Result<sqlx::PgPool, ErrorStatus> {
    let url = std::env::var("DATABASE_URL").map_err(|_| {
        ErrorStatus::from((
            StatusCode::INTERNAL_SERVER_ERROR,
            "DATABASE_URL is not set".to_string(),
        ))
    })?;
    sqlx::PgPool::connect(&url).await.map_err(|e| {
        ErrorStatus::from((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to connect to database: {e}"),
        ))
    })
}

fn verify_token(params: &HashMap<String, String>) -> Result<(), ErrorStatus> {
    let token = params.get("token").ok_or_else(|| {
        ErrorStatus::from((
            StatusCode::UNAUTHORIZED,
            "Missing token parameter".to_string(),
        ))
    })?;
    let expected = std::env::var("EMUMET_TEST_MODE_TOKEN")
        .unwrap_or_else(|_| "unsafe-default-test-token".to_string());
    if token != &expected {
        return Err(ErrorStatus::from((
            StatusCode::UNAUTHORIZED,
            "Invalid token".to_string(),
        )));
    }
    Ok(())
}

pub trait TestModeRouter {
    fn route_test_mode(self) -> Self;
}

impl TestModeRouter for Router<crate::handler::AppModule> {
    fn route_test_mode(self) -> Self {
        self.route("/__test__/health", get(health))
            .route("/__test__/reset", post(reset))
            .route("/__test__/inbox", get(inbox))
    }
}

async fn health() -> StatusCode {
    StatusCode::OK
}

async fn reset(Query(params): Query<HashMap<String, String>>) -> Result<StatusCode, ErrorStatus> {
    verify_token(&params)?;
    let pool = get_pool().await?;

    sqlx::query(
        "TRUNCATE accounts, account_events, auth_accounts, auth_account_events, \
         auth_emumet_accounts, profiles, profile_events, metadatas, metadata_events, \
         auth_hosts, follows, remote_accounts, images, signing_keys, outbox_activities \
         CASCADE",
    )
    .execute(&pool)
    .await
    .map_err(|e| {
        ErrorStatus::from((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to truncate tables: {e}"),
        ))
    })?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct InboxActivity {
    id: String,
    activity_type: String,
    actor: Option<String>,
    object: Option<String>,
    created_at: String,
}

#[derive(Serialize)]
struct InboxResponse {
    activities: Vec<InboxActivity>,
}

async fn inbox(
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<InboxResponse>, ErrorStatus> {
    verify_token(&params)?;
    let pool = get_pool().await?;

    use sqlx::Row;

    let rows = sqlx::query(
        "SELECT id, activity_type, object_json, created_at \
         FROM outbox_activities \
         ORDER BY id DESC \
         LIMIT 100",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| {
        ErrorStatus::from((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to query inbox activities: {e}"),
        ))
    })?;

    let activities: Vec<InboxActivity> = rows
        .iter()
        .map(|row| {
            let id: i64 = row.get("id");
            let activity_type: String = row.get("activity_type");
            let object_json: String = row.get("object_json");
            let created_at: time::OffsetDateTime = row.get("created_at");

            let (actor, object) = serde_json::from_str::<serde_json::Value>(&object_json)
                .ok()
                .map(|v| {
                    (
                        v.get("actor").and_then(|a| a.as_str()).map(String::from),
                        v.get("object").and_then(|o| {
                            o.as_str().map(String::from).or_else(|| {
                                o.get("id").and_then(|id| id.as_str()).map(String::from)
                            })
                        }),
                    )
                })
                .unwrap_or((None, None));

            InboxActivity {
                id: id.to_string(),
                activity_type,
                actor,
                object,
                created_at: created_at
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
            }
        })
        .collect();

    Ok(Json(InboxResponse { activities }))
}
