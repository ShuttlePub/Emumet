//! `GET /healthz`: "the HTTP listener is up and the router answers", nothing more.
//!
//! Meant for container start-up checks. It is deliberately NOT a readiness probe: it does not
//! touch the database, JWKS / Hydra / Kratos / Keto, object storage or the master key, and says
//! nothing about the registration gate. The body is a fixed `ok\n` (no version, no
//! configuration, no internal state), no authentication is required, and the route is not part of
//! the OpenAPI document (`openapi.json` must not change because of it).

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

/// Exactly these bytes, nothing else.
pub(crate) const HEALTHZ_BODY: &str = "ok\n";

pub trait HealthzRouter {
    fn route_healthz(self) -> Self;
}

impl<S> HealthzRouter for axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn route_healthz(self) -> Self {
        self.route("/healthz", axum::routing::get(healthz))
    }
}

async fn healthz() -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        HEALTHZ_BODY,
    )
        .into_response()
}

#[cfg(test)]
#[path = "healthz_tests.rs"]
mod tests;
