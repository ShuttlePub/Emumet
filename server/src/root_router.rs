//! Construction of the production root router.
//!
//! Extracted from `main` so that tests can build the very same router the server serves and
//! assert which routes exist on it (for example that the Profile actor / relationship routes of
//! P1-P3 are absent until the P4 cutover mounts them).

use crate::api;
use crate::auth::{self, JwksCache, OidcConfig};
use crate::handler::AppModule;
use crate::route::account::{AccountRouter, AdminAccountRouter, OrgAccountRouter};
use crate::route::activitypub::{ActivityPubRouter, FederationRouter};
use crate::route::me::MeRouter;
use crate::route::media::MediaRouter;
use crate::route::oauth2::OAuth2Router;
use crate::route::profile_transfer::ProfileTransferRouter;
use crate::route::profile_username_availability::ProfileUsernameAvailabilityRouter;
use crate::route::registration_gate::AdminRegistrationGateRouter;
use crate::route::registration_gate_webhook::AdminRegistrationGateWebhookRouter;
use crate::route::registration_protection::{
    AdminRegistrationProtectionRouter, RegistrationEmailRouter,
};
use crate::route::report::{AdminReportRouter, ReportRouter};
use crate::route::signing::SigningRouter;
#[cfg(feature = "test-mode")]
use crate::route::test_mode::TestModeRouter;
use axum::http::{header, HeaderValue, Method};
use std::sync::Arc;
use tower_http::cors::CorsLayer;

pub(crate) fn build_root_router(
    app: AppModule,
    oidc_config: Arc<OidcConfig>,
    jwks_cache: Arc<JwksCache>,
) -> axum::Router {
    // Routes that require JWT auth (/api/v1, /api/v1/admin, /internal/v1).
    // Admin authorization (Keto instance_moderate) lives inside the use cases.
    let api_v1 = axum::Router::new()
        .route_account()
        .route_org_account()
        .route_profile_username_availability()
        .route_profile_transfers()
        .route_reports()
        .route_me()
        .route_media()
        .nest(
            "/admin",
            axum::Router::new()
                .route_admin_account()
                .route_admin_reports()
                .route_admin_registration_gate()
                .route_admin_registration_gate_webhook()
                .route_admin_registration_protection(),
        );

    let authed_routes = axum::Router::new()
        .nest("/api/v1", api_v1)
        .nest("/internal/v1", axum::Router::new().route_signing())
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            api::organization_context::organization_context_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            (oidc_config, jwks_cache),
            auth::auth_middleware,
        ));

    // Routes that do NOT require JWT auth (OAuth2 Login/Consent Provider,
    // webfinger, federation under /ap — inbox is HTTP-Signature guarded)
    let public_routes = axum::Router::new()
        .route_oauth2()
        .route_activitypub()
        .nest("/ap", axum::Router::new().route_federation())
        .route_registration_email();

    #[cfg(feature = "test-mode")]
    let public_routes = public_routes.route_test_mode();

    authed_routes
        .merge(public_routes)
        .layer(build_cors_layer())
        .with_state(app)
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
                .allow_credentials(true)
                .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
                .allow_headers([
                    header::AUTHORIZATION,
                    header::CONTENT_TYPE,
                    header::HeaderName::from_static("x-organization-id"),
                ])
        }
    }
}
