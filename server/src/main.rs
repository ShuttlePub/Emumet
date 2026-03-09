mod applier;
mod error;
mod handler;
mod keycloak;
mod permission;
mod route;

use crate::error::StackTrace;
use crate::handler::AppModule;
use crate::keycloak::create_keycloak_instance;
use crate::route::account::AccountRouter;
use crate::route::metadata::MetadataRouter;
use crate::route::profile::ProfileRouter;
use error_stack::ResultExt;
use kernel::KernelError;
use std::net::SocketAddr;
use std::sync::Arc;
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

    let keycloak_auth_instance = Arc::new(create_keycloak_instance());
    let app = AppModule::new().await?;

    let router = axum::Router::new()
        .route_account(keycloak_auth_instance.clone())
        .route_profile(keycloak_auth_instance.clone())
        .route_metadata(keycloak_auth_instance)
        .layer(CorsLayer::new())
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
