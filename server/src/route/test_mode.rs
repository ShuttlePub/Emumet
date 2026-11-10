use crate::error::ErrorStatus;
use application::service::activitypub::inject_test_remote_actor;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use driver::http_signing::inject_test_actor_key;
use driver::profile_delivery::inject_test_resolved_actor;
use serde::Deserialize;

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

fn verify_token(headers: &HeaderMap) -> Result<(), ErrorStatus> {
    let token = headers
        .get("x-emumet-test-token")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            ErrorStatus::from((
                StatusCode::UNAUTHORIZED,
                "Missing X-Emumet-Test-Token header".to_string(),
            ))
        })?;
    let expected = std::env::var("EMUMET_TEST_MODE_TOKEN").map_err(|_| {
        ErrorStatus::from((
            StatusCode::INTERNAL_SERVER_ERROR,
            "EMUMET_TEST_MODE_TOKEN is not set".to_string(),
        ))
    })?;
    if token != expected {
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
            .route("/__test__/cache-actor-key", post(cache_actor_key))
            .route("/__test__/cache-remote-actor", post(cache_remote_actor))
    }
}

async fn health() -> StatusCode {
    StatusCode::OK
}

async fn reset(headers: HeaderMap) -> Result<StatusCode, ErrorStatus> {
    verify_token(&headers)?;
    let pool = get_pool().await?;

    sqlx::query(
        "TRUNCATE accounts, account_events, auth_accounts, auth_emumet_accounts, \
         profiles, profile_events, metadatas, metadata_events, projection_checkpoints, \
         auth_hosts, follows, remote_accounts, images, signing_keys, outbox_activities, \
         profile_signing_keys, profile_follows, profile_blocks, profile_mutes, \
         profile_inbound_activities, profile_delivery_jobs, profile_delivery_attempts \
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

#[derive(Deserialize)]
struct CacheActorKeyRequest {
    key_id: String,
    public_key_pem: String,
    /// Optional ActivityPub actor ID that owns this key. When omitted or empty,
    /// the owner is derived from the key_id URL by stripping the fragment.
    owner: Option<String>,
}

/// Inject an actor public key into the global test cache so that the HTTP
/// Signature verifier returns it without making a remote fetch.
///
/// This is needed for E2E tests against instances (e.g. Iceshrimp) whose
/// ActivityPub actor endpoints require authentication.
#[cfg(feature = "test-mode")]
async fn cache_actor_key(
    headers: HeaderMap,
    Json(body): Json<CacheActorKeyRequest>,
) -> Result<StatusCode, ErrorStatus> {
    verify_token(&headers)?;
    inject_test_actor_key(
        &body.key_id,
        body.public_key_pem,
        body.owner.as_deref().unwrap_or(""),
    );
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct CacheRemoteActorRequest {
    actor_url: String,
    inbox_url: String,
}

/// Inject a remote actor's inbox into the test global caches so that actor resolution (inbound
/// follower lookup and outbound Follow target lookup) returns it without making a remote fetch.
///
/// This is needed for E2E tests against instances whose ActivityPub actor
/// endpoints require authentication (e.g. Iceshrimp).
#[cfg(feature = "test-mode")]
async fn cache_remote_actor(
    headers: HeaderMap,
    Json(body): Json<CacheRemoteActorRequest>,
) -> Result<StatusCode, ErrorStatus> {
    verify_token(&headers)?;
    inject_test_remote_actor(&body.actor_url, &body.inbox_url);
    inject_test_resolved_actor(&body.actor_url, &body.inbox_url);
    Ok(StatusCode::NO_CONTENT)
}
