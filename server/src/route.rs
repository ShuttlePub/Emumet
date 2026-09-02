use crate::error::ErrorStatus;
use application::dto::pagination::Direction;
use axum::http::StatusCode;

pub mod account;
pub mod activitypub;
pub mod me;
pub mod media;
pub mod oauth2;
pub mod report;
pub mod signing;

#[cfg(feature = "test-mode")]
pub mod test_mode;

const MAX_BATCH_SIZE: usize = 100;

fn parse_comma_ids(raw: &str) -> Result<Vec<String>, ErrorStatus> {
    let ids: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "ID list cannot be empty".to_string(),
        )));
    }
    if ids.len() > MAX_BATCH_SIZE {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            format!("Too many IDs: maximum is {MAX_BATCH_SIZE}"),
        )));
    }
    Ok(ids)
}

trait DirectionConverter {
    fn convert_to_direction(self) -> Result<Direction, ErrorStatus>;
}

impl DirectionConverter for Option<String> {
    fn convert_to_direction(self) -> Result<Direction, ErrorStatus> {
        match self {
            Some(d) => match Direction::try_from(d) {
                Ok(d) => Ok(d),
                Err(message) => Err((StatusCode::BAD_REQUEST, message).into()),
            },
            None => Ok(Direction::default()),
        }
    }
}

#[cfg(test)]
pub(crate) fn build_test_router(app: crate::handler::AppModule) -> axum::Router {
    use crate::auth::{JwksCache, OidcConfig};
    let oidc_config = std::sync::Arc::new(OidcConfig {
        issuer_url: "http://localhost:4444".to_string(),
        expected_audience: "emumet".to_string(),
        jwks_refetch_interval_secs: 0,
    });
    let jwks_cache = std::sync::Arc::new(JwksCache::new(
        oidc_config.issuer_url.clone(),
        std::time::Duration::from_secs(0),
    ));

    build_test_router_with_auth(app, oidc_config, jwks_cache)
}

#[cfg(test)]
pub(crate) fn build_test_router_with_auth(
    app: crate::handler::AppModule,
    oidc_config: std::sync::Arc<crate::auth::OidcConfig>,
    jwks_cache: std::sync::Arc<crate::auth::JwksCache>,
) -> axum::Router {
    use crate::route::account::{AccountRouter, AdminAccountRouter, OrgAccountRouter};
    use crate::route::activitypub::{ActivityPubRouter, FederationRouter};
    use crate::route::me::MeRouter;
    use crate::route::media::MediaRouter;
    use crate::route::oauth2::OAuth2Router;
    use crate::route::report::{AdminReportRouter, ReportRouter};
    use crate::route::signing::SigningRouter;

    let api_v1 = axum::Router::new()
        .route_account()
        .route_org_account()
        .route_reports()
        .route_me()
        .route_media()
        .nest(
            "/admin",
            axum::Router::new()
                .route_admin_account()
                .route_admin_reports(),
        );

    let authed_routes = axum::Router::new()
        .nest("/api/v1", api_v1)
        .nest("/internal/v1", axum::Router::new().route_signing())
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            crate::api::organization_context::organization_context_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            (oidc_config, jwks_cache),
            crate::auth::auth_middleware,
        ));

    let public_routes = axum::Router::new()
        .route_oauth2()
        .route_activitypub()
        .nest("/ap", axum::Router::new().route_federation());

    authed_routes.merge(public_routes).with_state(app)
}

#[cfg(test)]
#[path = "route/tests.rs"]
mod route_smoke_tests;
