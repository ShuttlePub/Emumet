//! Username availability check for Profile creation forms (advisory, read-only).
//!
//! `GET /api/v1/profiles/username-availability?username=...` answers whether a username could be
//! assigned right now, with the reason when it cannot. The rules and the "already used" source are
//! the ones the creation / assignment APIs enforce, but the answer is only a hint: those APIs
//! (400 / 409) decide, because another request may take the username after this check.

mod rate_limiter;

use crate::api::profile_username_availability::ProfileUsernameAvailabilityApi;
use crate::auth::AuthClaims;
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::schema::profile_username_availability::{
    ProfileUsernameAvailabilityQuery, ProfileUsernameAvailabilityResponse,
    ProfileUsernameUnavailableReason,
};
use application::service::profile_username_availability::ProfileUsernameAvailability;
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use rate_limiter::{RateLimitDecision, UsernameAvailabilityRateLimiter};
use std::sync::Arc;

pub(crate) enum UsernameAvailabilityError {
    RateLimited { retry_after_secs: u64 },
    Internal(ErrorStatus),
}

impl IntoResponse for UsernameAvailabilityError {
    fn into_response(self) -> Response {
        match self {
            UsernameAvailabilityError::RateLimited { retry_after_secs } => {
                let mut response = (
                    StatusCode::TOO_MANY_REQUESTS,
                    "Too many username availability checks; retry later",
                )
                    .into_response();
                if let Ok(value) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                    response.headers_mut().insert(header::RETRY_AFTER, value);
                }
                response
            }
            UsernameAvailabilityError::Internal(error) => error.into_response(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/profiles/username-availability",
    description = "Check whether a Profile username could be assigned, for input-time validation. The answer is ADVISORY: it is a snapshot, nothing is reserved (no hold), and another request may take the username before the form is submitted, so the final decision is always made by the create / set username APIs (400 for a malformed or reserved value, 409 when the username is taken).\n\n\
The rules are the same as those applied by the create / set username APIs: `^[a-z0-9][a-z0-9._-]{0,63}$` (ASCII lowercase letters, digits, `.`, `_`, `-`; 1-64 characters), a reserved-word list, and no implicit conversion (no trimming or case folding: `Alice` is `invalid_format`, not `alice`). A username is unique across all Profiles, also deleted ones, and is never reused.\n\n\
Always 200 for a well-formed request: `available` is true, or false with `reason` = `invalid_format`, `reserved` or `taken`. `taken` covers both \"in use\" and \"used by a deleted Profile\" and does not tell them apart (so the response reveals neither whether a Profile was deleted nor anything else about its owner). A missing `username` parameter is 400. Read-only: nothing is created or stored.\n\n\
Rate limited per authenticated caller (token `iss` + `sub`): 60 requests per minute, then 429 with `Retry-After` (seconds). The limit is kept in memory per server instance and is not shared between instances. Changing a username is not supported.",
    params(
        ("username" = String, Query, description = "Username to check, exactly as the user typed it (no trimming or case folding is applied)"),
    ),
    responses(
        (status = 200, description = "Availability result", body = ProfileUsernameAvailabilityResponse),
        (status = 400, description = "The username query parameter is missing"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 429, description = "Too many checks; see the Retry-After header (seconds)"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn get_profile_username_availability(
    Extension(claims): Extension<AuthClaims>,
    Extension(limiter): Extension<Arc<UsernameAvailabilityRateLimiter>>,
    State(api): State<ProfileUsernameAvailabilityApi>,
    Query(query): Query<ProfileUsernameAvailabilityQuery>,
) -> Result<Json<ProfileUsernameAvailabilityResponse>, UsernameAvailabilityError> {
    if let RateLimitDecision::Limited { retry_after } = limiter.check(&claims.iss, &claims.sub) {
        return Err(UsernameAvailabilityError::RateLimited {
            retry_after_secs: retry_after.as_secs_f64().ceil().max(1.0) as u64,
        });
    }
    let availability = api
        .check(&query.username)
        .await
        .map_err(|e| UsernameAvailabilityError::Internal(ErrorStatus::from(e)))?;
    let reason = match availability {
        ProfileUsernameAvailability::Available => None,
        ProfileUsernameAvailability::InvalidFormat => {
            Some(ProfileUsernameUnavailableReason::InvalidFormat)
        }
        ProfileUsernameAvailability::Reserved => Some(ProfileUsernameUnavailableReason::Reserved),
        ProfileUsernameAvailability::Taken => Some(ProfileUsernameUnavailableReason::Taken),
    };
    Ok(Json(ProfileUsernameAvailabilityResponse {
        username: query.username,
        available: reason.is_none(),
        reason,
    }))
}

pub trait ProfileUsernameAvailabilityRouter {
    /// Routes relative to `/api/v1`. Every call gets its own rate limiter.
    fn route_profile_username_availability(self) -> Self;
}

impl ProfileUsernameAvailabilityRouter for Router<AppModule> {
    fn route_profile_username_availability(self) -> Self {
        let limiter = Arc::new(UsernameAvailabilityRateLimiter::default());
        self.route(
            "/profiles/username-availability",
            get(get_profile_username_availability).layer(Extension(limiter)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rate_limited_response_is_429_with_retry_after() {
        let response = UsernameAvailabilityError::RateLimited {
            retry_after_secs: 7,
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "7");
    }
}
