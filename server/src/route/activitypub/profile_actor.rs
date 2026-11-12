//! Profile ActivityPub actor document and WebFinger handlers (P1).
//!
//! 1 Profile = 1 actor. The final URL shape (fixed by P1, ADR profile-actor-identity):
//!
//! * actor id / `url` / `publicKey.owner`: `{PUBLIC_BASE_URL}/ap/profiles/{profile_nanoid}`
//! * inbox / outbox / followers / following: that URL + `/inbox` `/outbox` `/followers` `/following`
//! * key id: that URL + `#main-key`
//! * acct: `username@{PUBLIC_BASE_URL host[:port]}` resolved by `/.well-known/webfinger`
//!
//! The nanoid (not the username) is the path component, so the actor URI is derived from the
//! immutable Profile identity and never changes on Profile transfer.
//!
//! These handlers are mounted on the production root router by the P4 cutover (together with the
//! retirement of the `/ap/accounts/...` routes, see `route::retired`) through [`ProfileActorRouter`].

use super::discovery::parse_webfinger_resource;
use super::{json_response, ACTIVITY_JSON, JRD_JSON};
use crate::api::ActivityPubApi;
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use application::dto::activitypub::{GetProfileActorDto, GetProfileWebFingerDto};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use std::collections::HashMap;

#[utoipa::path(
    get,
    path = "/ap/profiles/{profile_nanoid}",
    description = "Profile ActivityPub actor document (1 Profile = 1 actor, ADR 0002). \
Actor id: {PUBLIC_BASE_URL}/ap/profiles/{profile_nanoid}; inbox/outbox/followers/following are that URL plus /inbox, /outbox, /followers, /following; the key id is that URL plus #main-key; preferredUsername is the Profile username. \
A Profile is only an actor once it has a username and a Profile signing key (404 otherwise); every Profile has both after the cutover. \
The public key published here is the only way to obtain a Profile key: there is no signing or key-export API.",
    params(("profile_nanoid" = String, Path, description = "Profile nanoid (external ID)")),
    responses(
        (status = 200, description = "ActivityPub actor document", body = kernel::activitypub::Actor, content_type = "application/activity+json"),
        (status = 404, description = "Profile not found, or it has no username / signing key yet"),
    ),
    tag = "ProfileActor",
)]
pub(crate) async fn get_profile_actor(
    State(api): State<ActivityPubApi>,
    Path(profile_nanoid): Path<String>,
) -> Result<Response, ErrorStatus> {
    if profile_nanoid.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Profile ID cannot be empty".to_string(),
        )));
    }
    let actor = api
        .get_profile_actor(GetProfileActorDto { profile_nanoid })
        .await
        .map_err(ErrorStatus::from)?;
    json_response(&actor, ACTIVITY_JSON)
}

/// WebFinger for Profile acct names (`acct:username@domain`). The Account based WebFinger it
/// replaced is gone: Account names no longer resolve.
#[utoipa::path(
    get,
    path = "/.well-known/webfinger",
    description = "WebFinger discovery of a Profile actor by acct name (`acct:username@{PUBLIC_BASE_URL host[:port]}`, domain compared case-insensitively). The `self` link is the Profile actor URL. Account names are not resolvable.",
    params(("resource" = String, Query, description = "Resource URI (acct:username@domain)")),
    responses(
        (status = 200, description = "WebFinger response", body = kernel::activitypub::WebFingerResponse, content_type = "application/jrd+json"),
        (status = 400, description = "Invalid resource format"),
        (status = 404, description = "No such Profile actor, or the domain is not this instance"),
    ),
    tag = "ProfileActor",
)]
pub(crate) async fn profile_webfinger(
    State(api): State<ActivityPubApi>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ErrorStatus> {
    let resource = params.get("resource").ok_or_else(|| {
        ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Missing resource parameter".to_string(),
        ))
    })?;
    let parsed = parse_webfinger_resource(resource)?;
    let expected_domain = api.public_base_host_header()?;
    if !parsed.domain.eq_ignore_ascii_case(&expected_domain) {
        return Err(ErrorStatus::from(StatusCode::NOT_FOUND));
    }
    let response = api
        .get_profile_webfinger(GetProfileWebFingerDto {
            username: parsed.account_name,
            domain: parsed.domain,
        })
        .await
        .map_err(ErrorStatus::from)?;
    json_response(&response, JRD_JSON)
}

/// Mounts the Profile actor handlers on the production root router.
pub trait ProfileActorRouter {
    /// Routes relative to `/ap`.
    fn route_profile_actor(self) -> Self;
    /// Root-level `/.well-known/webfinger` for Profile acct names.
    fn route_profile_webfinger(self) -> Self;
}

impl ProfileActorRouter for Router<AppModule> {
    fn route_profile_actor(self) -> Self {
        self.route("/profiles/{profile_nanoid}", get(get_profile_actor))
    }

    fn route_profile_webfinger(self) -> Self {
        self.route("/.well-known/webfinger", get(profile_webfinger))
    }
}

#[cfg(test)]
#[path = "profile_actor_tests.rs"]
mod tests;
