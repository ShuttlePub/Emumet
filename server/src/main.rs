mod api;
mod auth;
mod cutover;
mod error;
mod handler;
mod instance_identity;
mod kratos;
mod logging;
mod openapi;
mod post_relay_cli;
mod post_relay_operator;
mod post_relay_worker;
mod projection_worker;
mod registration_guard;
mod root_router;
mod route;
mod runtime_config;
mod schema;
mod shutdown;
mod webhook_worker;

use crate::api::profile_delivery_worker::{
    profile_delivery_poll_interval_from_env, spawn_profile_delivery_worker,
};
use crate::auth::{JwksCache, OidcConfig};
use crate::error::StackTrace;
use crate::handler::AppModule;
use crate::projection_worker::{projection_poll_interval_from_env, ProjectionWorker};
use crate::shutdown::ShutdownSignals;
use error_stack::ResultExt;
use kernel::prelude::entity::ProfileDeliveryPolicy;
use kernel::KernelError;
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), StackTrace> {
    if post_relay_cli::dispatch(std::env::args().skip(1)).await? {
        return Ok(());
    }
    // Created before anything slow so that a SIGTERM / SIGINT during start-up is not lost; polled
    // at the points below and by the graceful shutdown of the HTTP server.
    let mut shutdown = ShutdownSignals::install()
        .change_context_lazy(|| KernelError::Internal)
        .attach_printable("Failed to install the SIGTERM / SIGINT handlers")?;

    let startup_started = std::time::Instant::now();
    // Keep the guard alive until the end of `main`: it flushes the optional log file.
    let _log_guard = logging::init()?;

    // Initialize Snowflake ID generator (must happen before any ID generation)
    let worker_id: u64 = std::env::var("WORKER_ID")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    kernel::init_generator(worker_id);
    tracing::info!(worker_id, "Snowflake ID generator initialized");

    // OIDC / JWT auth setup
    let limits = runtime_config::RuntimeLimits::from_env()?;
    let oidc_config = OidcConfig::from_env();
    let jwks_cache = Arc::new(JwksCache::with_timeout(
        oidc_config.issuer_url.clone(),
        Duration::from_secs(oidc_config.jwks_refetch_interval_secs),
        limits.jwks_timeout,
    ));
    let oidc_config = Arc::new(oidc_config);

    // Applies the migrations (the in-place cutover migration included), then generates the actor
    // keys of Profiles that have none. Both happen before any worker or route is started.
    let app = AppModule::new().await?;
    cutover::provision_profile_keys(&app).await?;
    // The instance actor identity and its key (post-relay signer). Nothing is repaired: a different
    // PUBLIC_BASE_URL or a key that cannot be decrypted stops the start-up here.
    instance_identity::provision_instance_identity(&app).await?;
    // The migrations and the key provisioning above are never interrupted half way. A stop request
    // that arrived meanwhile is honoured now, before any worker or listener exists.
    if shutdown.is_requested() {
        tracing::info!("Stop requested during start-up; exiting before serving");
        return Ok(());
    }

    // Reserve the listener only after migrations and both key checks; bind errors cannot leave
    // a sending worker running. No Ory call is a prerequisite for this independent runtime.
    let bind = std::env::var("HTTP_LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
        .parse::<SocketAddr>()
        .map_err(|_| error_stack::Report::new(KernelError::Validation))?;
    let tcp = TcpListener::bind(bind)
        .await
        .change_context_lazy(|| KernelError::Internal)
        .attach_printable_lazy(|| format!("Failed to bind HTTP listener to {bind}"))?;

    let jwks_warmup = jwks_cache.clone().spawn_warmup();

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

    let relay_sender_enabled = post_relay_worker::enabled()?;
    if !relay_sender_enabled {
        tracing::warn!("post relay sending disabled; retention maintenance remains active");
    }
    let (relay_stop, relay_handle) =
        post_relay_worker::spawn(app.post_relay_module(), relay_sender_enabled);
    let router = root_router::build_root_router(app, oidc_config, jwks_cache);

    let (http_stop, http_stopped) = tokio::sync::oneshot::channel::<()>();
    let mut serving = Box::pin(
        axum::serve(
            tcp,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = http_stopped.await;
        })
        .into_future(),
    );
    tracing::info!(address = %bind, elapsed_ms = startup_started.elapsed().as_millis(), "Independent HTTP and relay runtime started");
    let serve_result = tokio::select! {
        () = shutdown.requested() => None,
        result = &mut serving => Some(result),
    };

    // Stop new claims at signal receipt, NOT after HTTP's graceful shutdown (a request can hang).
    // Already claimed/authorized work retains S1's bounded drain and original fenced lease.
    tracing::info!(
        grace_secs = limits.shutdown_timeout.as_secs(),
        "Stopping claims and draining runtime"
    );
    let _ = relay_stop.send(true);
    let _ = http_stop.send(());
    jwks_warmup.abort();
    projection_shutdown.trigger();
    enforcement_shutdown.trigger();
    if let Some(shutdown) = registration_shutdown {
        shutdown.trigger();
    }
    profile_delivery_shutdown.trigger();
    let drained = tokio::time::timeout(limits.shutdown_timeout, async {
        let http_drain = async {
            match serve_result {
                Some(result) => result,
                None => (&mut serving).await,
            }
        };
        let (http, relay, webhook) = tokio::join!(
            http_drain,
            relay_handle,
            webhook_shutdown.stop_and_wait(webhook_worker::STOP_GRACE),
        );
        if relay.is_err() {
            tracing::warn!("Post relay worker stopped unexpectedly");
        }
        if !webhook {
            tracing::warn!("Registration webhook drain deadline exceeded");
        }
        http
    })
    .await;
    match drained {
        Ok(result) => result.change_context_lazy(|| KernelError::Internal)?,
        Err(_) => {
            tracing::warn!("Runtime drain deadline exceeded; fenced leases recover after expiry")
        }
    }
    Ok(())
}
