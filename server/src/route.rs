use crate::error::ErrorStatus;
use application::dto::pagination::Direction;
use axum::http::StatusCode;

pub mod account;
pub mod activitypub;
pub mod me;
pub mod media;
pub mod oauth2;
pub mod profile_domain_control;
pub mod profile_follow_list;
pub mod profile_policy;
pub mod profile_transfer;
pub mod registration_gate;
pub mod registration_gate_webhook;
pub mod report;
pub mod signing;

#[cfg(feature = "test-mode")]
pub mod test_mode;

#[cfg(test)]
pub(crate) mod profile_graph_test_support;

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
    // Same composition as the production server: tests (including the route-absence tests for
    // the not-yet-mounted Profile routes) must see exactly the routes that production serves.
    crate::root_router::build_root_router(app, oidc_config, jwks_cache)
}

#[cfg(test)]
#[path = "route/tests.rs"]
mod route_smoke_tests;
