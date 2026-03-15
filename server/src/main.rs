mod applier;
mod auth;
mod error;
mod handler;
mod hydra;
mod kratos;
mod openapi;
mod route;
mod schema;

use crate::auth::{JwksCache, OidcConfig};
use crate::error::StackTrace;
use crate::handler::AppModule;
use crate::route::account::AccountRouter;
use crate::route::metadata::MetadataRouter;
use crate::route::oauth2::OAuth2Router;
use crate::route::profile::ProfileRouter;
use axum::http::{header, HeaderValue, Method};
use error_stack::ResultExt;
use kernel::KernelError;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

#[tokio::main]
async fn main() -> Result<(), StackTrace> {
    let appender = tracing_appender::rolling::daily(std::path::Path::new("./logs/"), "debug.log");
    let (non_blocking_appender, _guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_filter(tracing_subscriber::EnvFilter::new(
                    std::env::var("RUST_LOG").unwrap_or_else(|_| {
                        "driver=debug,server=debug,tower_http=debug,hyper=debug,sqlx=debug".into()
                    }),
                ))
                .with_filter(tracing_subscriber::filter::LevelFilter::DEBUG),
        )
        .with(
            tracing_subscriber::fmt::Layer::default()
                .with_writer(non_blocking_appender)
                .with_ansi(false)
                .with_filter(tracing_subscriber::filter::LevelFilter::DEBUG),
        )
        .init();

    // Initialize Snowflake ID generator (must happen before any ID generation)
    let worker_id: u64 = std::env::var("WORKER_ID")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    kernel::init_generator(worker_id);
    tracing::info!(worker_id, "Snowflake ID generator initialized");

    // OIDC / JWT auth setup
    let oidc_config = OidcConfig::from_env();
    let jwks_cache = Arc::new(JwksCache::new(
        oidc_config.issuer_url.clone(),
        Duration::from_secs(oidc_config.jwks_refetch_interval_secs),
    ));
    // Attempt eager JWKS init (non-fatal if Hydra is not yet available).
    jwks_cache.try_init().await;
    let oidc_config = Arc::new(oidc_config);

    let app = AppModule::new().await?;

    // Routes that require JWT auth
    let authed_routes = axum::Router::new()
        .route_account()
        .route_profile()
        .route_metadata()
        .layer(axum::middleware::from_fn_with_state(
            (oidc_config, jwks_cache),
            auth::auth_middleware,
        ));

    // Routes that do NOT require JWT auth (OAuth2 Login/Consent Provider)
    let public_routes = axum::Router::new().route_oauth2();

    let router = authed_routes
        .merge(public_routes)
        .layer(build_cors_layer())
        .with_state(app);

    let bind = SocketAddr::from(([0, 0, 0, 0], 8080));
    let tcp = TcpListener::bind(bind)
        .await
        .change_context_lazy(|| KernelError::Internal)
        .attach_printable_lazy(|| "Failed to bind to port 8080")?;

    axum::serve(tcp, router.into_make_service())
        .await
        .change_context_lazy(|| KernelError::Internal)?;

    Ok(())
}

fn build_cors_layer() -> CorsLayer {
    match std::env::var("CORS_ALLOWED_ORIGINS").ok().as_deref() {
        None | Some("*") => CorsLayer::permissive(),
        Some(origins) => {
            let origins: Vec<HeaderValue> = origins
                .split(',')
                .filter_map(|s| s.trim().parse::<HeaderValue>().ok())
                .collect();
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
                .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        }
    }
}
