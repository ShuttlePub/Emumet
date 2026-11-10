mod api;
mod auth;
mod cutover;
mod error;
mod handler;
mod hydra;
mod kratos;
mod openapi;
mod projection_worker;
mod registration_guard;
mod root_router;
mod route;
mod schema;
mod webhook_worker;

use crate::api::profile_delivery_worker::{
    profile_delivery_poll_interval_from_env, spawn_profile_delivery_worker,
};
use crate::auth::{JwksCache, OidcConfig};
use crate::error::StackTrace;
use crate::handler::AppModule;
use crate::projection_worker::{projection_poll_interval_from_env, ProjectionWorker};
use error_stack::ResultExt;
use kernel::prelude::entity::ProfileDeliveryPolicy;
use kernel::KernelError;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
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

    // Applies the migrations (the in-place cutover migration included), then generates the actor
    // keys of Profiles that have none. Both happen before any worker or route is started.
    let app = AppModule::new().await?;
    cutover::provision_profile_keys(&app).await?;

    // Transactional log tailing worker for account projections (ADR 0006 Stage 3).
    let (_projection_handle, projection_shutdown) =
        ProjectionWorker::spawn(Arc::new(app.clone()), projection_poll_interval_from_env());

    if webhook_worker::prepare_baseline(&app).await.is_err() {
        tracing::warn!("registration webhook baseline could not be prepared; worker will retry");
    }
    let webhook_shutdown = webhook_worker::spawn(Arc::new(app.clone()));
    // Invalid hook configuration must not take login/Account APIs down with registration.
    let registration_shutdown =
        match registration_guard::spawn_if_enabled(Arc::new(app.clone())).await {
            Ok(shutdown) => shutdown,
            Err(_) => {
                app.invalidate_registration_enforcement().await?;
                tracing::error!("registration guard could not start; registration gate stopped");
                None
            }
        };
    use application::service::registration_enforcement_probe::RegistrationEnforcementMonitorUseCase;
    app.refresh_registration_enforcement().await?;
    let enforcement_shutdown = registration_guard::spawn_monitor(Arc::new(app.clone()));

    let (profile_delivery_shutdown, _profile_delivery_handle) = spawn_profile_delivery_worker(
        Arc::new(app.clone()),
        ProfileDeliveryPolicy::default(),
        profile_delivery_poll_interval_from_env(),
    );

    #[cfg(feature = "test-mode")]
    {
        let token = std::env::var("EMUMET_TEST_MODE_TOKEN");
        if token.as_deref().unwrap_or("").is_empty() {
            eprintln!(
                "FATAL: EMUMET_TEST_MODE_TOKEN must be set when running with test-mode feature"
            );
            std::process::exit(1);
        }
    }

    let router = root_router::build_root_router(app, oidc_config, jwks_cache);

    let bind = std::env::var("HTTP_LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
        .parse::<SocketAddr>()
        .map_err(|_| error_stack::Report::new(KernelError::Validation))?;
    let tcp = TcpListener::bind(bind)
        .await
        .change_context_lazy(|| KernelError::Internal)
        .attach_printable_lazy(|| format!("Failed to bind HTTP listener to {bind}"))?;

    axum::serve(
        tcp,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
    .change_context_lazy(|| KernelError::Internal)?;

    projection_shutdown.trigger();
    enforcement_shutdown.trigger();
    if let Some(shutdown) = registration_shutdown {
        shutdown.trigger();
    }
    profile_delivery_shutdown.trigger();
    webhook_shutdown.trigger();

    Ok(())
}
