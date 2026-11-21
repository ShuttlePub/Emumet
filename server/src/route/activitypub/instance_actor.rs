//! The Emumet instance actor document (P1 post-relay, K1).
//!
//! `{PUBLIC_BASE_URL}/ap/instance` is an `Application` actor, not a Profile. It publishes the public
//! key(s) of the signer of the requests Emumet sends to a ShuttlePub home. Anonymous GET; the actor
//! id and every key id come from the stored identity that is checked against the configured
//! `PUBLIC_BASE_URL`: the request `Host` is never used.

use super::{json_response, ACTIVITY_JSON};
use crate::api::ActivityPubApi;
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;

#[utoipa::path(
    get,
    path = "/ap/instance",
    description = "The Emumet instance actor (type Application), the signer of post-relay requests to a ShuttlePub home. \
Actor id: {PUBLIC_BASE_URL}/ap/instance; publicKey is an array (several keys can be published at once), each key id is {PUBLIC_BASE_URL}/ap/instance#key-{opaque-id} and its owner is the instance actor. \
Public keys only: the private key is never exposed. The URLs come from the configured PUBLIC_BASE_URL, not from the request Host. Anonymous. \
A stored identity that does not match the configuration is a 500, never a regenerated one.",
    responses(
        (status = 200, description = "Instance actor document", body = kernel::activitypub::InstanceActor, content_type = "application/activity+json"),
        (status = 500, description = "The instance identity is missing or inconsistent with the configuration"),
    ),
    tag = "InstanceActor",
)]
pub(crate) async fn get_instance_actor(
    State(api): State<ActivityPubApi>,
) -> Result<Response, ErrorStatus> {
    let actor = api.get_instance_actor().await.map_err(ErrorStatus::from)?;
    json_response(&actor, ACTIVITY_JSON)
}

/// Mounts the instance actor on the production root router.
pub trait InstanceActorRouter {
    /// Route relative to `/ap`.
    fn route_instance_actor(self) -> Self;
}

impl InstanceActorRouter for Router<AppModule> {
    fn route_instance_actor(self) -> Self {
        self.route("/instance", get(get_instance_actor))
    }
}

#[cfg(test)]
#[path = "instance_actor_tests.rs"]
mod tests;
