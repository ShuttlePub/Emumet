//! Construction of the production root router.
//!
//! Extracted from `main` so that tests can build the very same router the server serves and
//! assert which routes exist on it. Since the P4 cutover it serves the Profile federation model:
//!
//! - mounted: Profile actor document, Profile WebFinger and Profile inbox (P1/P3i), the Profile
//!   identity read, Follow / unfollow, individual block / mute and the followers / following
//!   reads (E8/P3o/P3i), the domain-controls routes (D8P storage; D8R enables block writes, which
//!   detach the existing relations with the blocked domain in the same transaction), the one-time
//!   username setter and the Account management API;
//! - retired (explicit `410 Gone`, `route::retired`): `/ap/accounts/...`, the Account signing and
//!   public-key endpoints, and the Account scoped Follow / block / mute operations.

use crate::api;
use crate::auth::{self, JwksCache, OidcConfig};
use crate::handler::AppModule;
use crate::route::account::{AccountRouter, AdminAccountRouter, OrgAccountRouter};
use crate::route::activitypub::profile_actor::ProfileActorRouter;
use crate::route::activitypub::profile_inbox::ProfileInboxRouter;
use crate::route::me::MeRouter;
use crate::route::media::MediaRouter;
use crate::route::oauth2::OAuth2Router;
use crate::route::profile_domain_control::DomainControlRouter;
use crate::route::profile_follow_list::ProfileFollowListRouter;
use crate::route::profile_relationship::ProfileRelationshipRouter;
use crate::route::profile_transfer::ProfileTransferRouter;
use crate::route::profile_username_availability::ProfileUsernameAvailabilityRouter;
use crate::route::registration_gate::AdminRegistrationGateRouter;
use crate::route::registration_gate_webhook::AdminRegistrationGateWebhookRouter;
use crate::route::registration_protection::{
    AdminRegistrationProtectionRouter, RegistrationEmailRouter,
};
use crate::route::report::{AdminReportRouter, ReportRouter};
use crate::route::retired::RetiredRouter;
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
        .route_profile_transfers()
        .route_reports()
        .route_me()
        .route_media()
        .route_profile_relationships()
        .route_profile_follow_lists()
        .route_domain_controls()
        .route_retired_account_relations()
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
        .nest("/internal/v1", axum::Router::new().route_retired_signing())
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            api::organization_context::organization_context_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            (oidc_config.clone(), jwks_cache.clone()),
            auth::auth_middleware,
        ));

    // JWT auth only, deliberately NOT behind the organization context middleware: that middleware
    // resolves the caller's auth account (find-or-create) when X-Organization-Id is present, and
    // the read-only username availability check must never create anything.
    let authed_without_organization_context = axum::Router::new()
        .nest(
            "/api/v1",
            axum::Router::new().route_profile_username_availability(),
        )
        .layer(axum::middleware::from_fn_with_state(
            (oidc_config, jwks_cache),
            auth::auth_middleware,
        ));

    // Routes that do NOT require JWT auth (OAuth2 Login/Consent Provider, Profile WebFinger,
    // Profile federation under /ap — the inbox is HTTP-Signature guarded, the retired Account
    // actor paths answer 410).
    let public_routes = axum::Router::new()
        .route_oauth2()
        .route_profile_webfinger()
        .nest(
            "/ap",
            axum::Router::new()
                .route_profile_actor()
                .route_profile_inbox()
                .route_retired_account_federation(),
        )
        .route_registration_email();

    #[cfg(feature = "test-mode")]
    let public_routes = public_routes.route_test_mode();

    authed_routes
        .merge(authed_without_organization_context)
        .merge(public_routes)
        .layer(build_cors_layer())
        .with_state(app)
}

fn build_cors_layer() -> CorsLayer {
    cors_layer_for(std::env::var("CORS_ALLOWED_ORIGINS").ok().as_deref())
}

fn cors_layer_for(allowed_origins: Option<&str>) -> CorsLayer {
    match allowed_origins {
        None | Some("*") => CorsLayer::permissive(),
        Some(origins) => {
            let origins: Vec<HeaderValue> = origins
                .split(',')
                .filter_map(|s| s.trim().parse::<HeaderValue>().ok())
                .collect();
            CorsLayer::new()
                .allow_origin(origins)
                .allow_credentials(true)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::PATCH,
                    Method::DELETE,
                ])
                .allow_headers([
                    header::AUTHORIZATION,
                    header::CONTENT_TYPE,
                    header::HeaderName::from_static("x-organization-id"),
                ])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn preflight(layer: CorsLayer, method: &str) -> axum::http::Response<Body> {
        axum::Router::new()
            .route("/api/v1/x", axum::routing::put(|| async { "ok" }))
            .layer(layer)
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/v1/x")
                    .header("origin", "https://app.example")
                    .header("access-control-request-method", method)
                    .header(
                        "access-control-request-headers",
                        "authorization,content-type,x-organization-id",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// The domain-controls routes use `PUT` and `DELETE`: both must pass a browser preflight in the
    /// restricted-origin configuration (the permissive default allows every method).
    #[tokio::test]
    async fn cors_allows_put_and_delete_for_the_configured_origins() {
        for method in ["PUT", "DELETE", "GET", "POST", "PATCH"] {
            let response = preflight(cors_layer_for(Some("https://app.example")), method).await;
            assert_eq!(response.status(), StatusCode::OK, "{method}");
            let allowed = response
                .headers()
                .get("access-control-allow-methods")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            assert!(
                allowed.contains(method),
                "{method} must be allowed, got {allowed}"
            );
            assert_eq!(
                response
                    .headers()
                    .get("access-control-allow-origin")
                    .map(|v| v.as_bytes()),
                Some(b"https://app.example".as_slice())
            );
        }
        let denied = preflight(cors_layer_for(Some("https://other.example")), "PUT").await;
        assert!(denied
            .headers()
            .get("access-control-allow-origin")
            .is_none());
        let permissive = preflight(cors_layer_for(None), "PUT").await;
        assert_eq!(permissive.status(), StatusCode::OK);
    }
}
