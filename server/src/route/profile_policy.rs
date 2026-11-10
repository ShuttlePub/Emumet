//! Contract-only Profile identity / relationship routes (E8).
//!
//! These handlers exist so that the OpenAPI document (`openapi.json`) can describe the future
//! contract. They are intentionally NOT part of the production root router (see
//! `ProfilePolicyContractRouter`, which only tests use) and every handler answers an explicit
//! `501 Not Implemented`. Real behaviour arrives in P1/P2/P3o/P3i/D8G/D8R and the
//! production mount in P4. The domain-controls routes were implemented by D8P and live in
//! `route::profile_domain_control`.

use crate::error::ErrorStatus;
use crate::schema::account::ProfileResponse;
use crate::schema::profile_policy::{
    CreateProfileFollowRequest, CreateProfileRelationRequest, ProfileFollowListResponse,
    ProfileFollowResponse, ProfileFollowerListResponse, ProfileRelationListResponse,
    ProfileRelationResponse,
};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::Router;

fn not_implemented(operation: &'static str) -> ErrorStatus {
    ErrorStatus::from((
        StatusCode::NOT_IMPLEMENTED,
        format!("{operation} is a contract-only skeleton and is not implemented or mounted yet"),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/profiles/{profile_nanoid}",
    description = "Profile identity read. NOT MOUNTED in production (contract only); the skeleton answers 501. \
Authorization: only the Profile's current owner (the owning personal Account, or an active member of the owning organization via X-Organization-Id). \
`username` is null while the Profile has no username (legacy Profiles; username is set once in P1 and is immutable afterwards). There is deliberately no username update API.",
    params(("profile_nanoid" = String, Path, description = "Profile nanoid (external ID)")),
    responses(
        (status = 200, description = "Profile identity", body = ProfileResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
        (status = 501, description = "Contract only: not implemented and not mounted on the production router"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn get_profile_identity() -> ErrorStatus {
    not_implemented("GET /api/v1/profiles/{profile_nanoid}")
}

#[utoipa::path(
    post,
    path = "/api/v1/profiles/{profile_nanoid}/follows",
    description = "Follow a target as the Profile (the Profile, not the Account, is the follower). NOT MOUNTED in production (contract only); the skeleton answers 501. \
Authorization: only the Profile's current owner. A new follow is rejected (422) when the target or its domain is blocked by the Profile or blocks the Profile. Mute does not reject follows.",
    params(("profile_nanoid" = String, Path, description = "Follower Profile nanoid")),
    request_body = CreateProfileFollowRequest,
    responses(
        (status = 201, description = "Follow recorded", body = ProfileFollowResponse),
        (status = 400, description = "Invalid target"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
        (status = 409, description = "Follow already exists"),
        (status = 422, description = "Follow refused by a block (individual or domain)"),
        (status = 501, description = "Contract only: not implemented and not mounted on the production router"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn create_profile_follow() -> ErrorStatus {
    not_implemented("POST /api/v1/profiles/{profile_nanoid}/follows")
}

#[utoipa::path(
    delete,
    path = "/api/v1/profiles/{profile_nanoid}/follows/{follow_id}",
    description = "Unfollow: remove a follow owned by the Profile. NOT MOUNTED in production (contract only); the skeleton answers 501. Authorization: only the Profile's current owner.",
    params(
        ("profile_nanoid" = String, Path, description = "Follower Profile nanoid"),
        ("follow_id" = String, Path, description = "Follow relation nanoid"),
    ),
    responses(
        (status = 204, description = "Follow removed"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile or follow not found"),
        (status = 501, description = "Contract only: not implemented and not mounted on the production router"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn delete_profile_follow() -> ErrorStatus {
    not_implemented("DELETE /api/v1/profiles/{profile_nanoid}/follows/{follow_id}")
}

#[utoipa::path(
    get,
    path = "/api/v1/profiles/{profile_nanoid}/following",
    description = "Profiles/actors followed by the Profile. NOT MOUNTED in production (contract only); the skeleton answers 501. Authorization: only the Profile's current owner.",
    params(
        ("profile_nanoid" = String, Path, description = "Profile nanoid"),
        ("limit" = Option<u32>, Query, description = "Pagination limit"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
    ),
    responses(
        (status = 200, description = "Following list", body = ProfileFollowListResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
        (status = 501, description = "Contract only: not implemented and not mounted on the production router"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn list_profile_following() -> ErrorStatus {
    not_implemented("GET /api/v1/profiles/{profile_nanoid}/following")
}

#[utoipa::path(
    get,
    path = "/api/v1/profiles/{profile_nanoid}/followers",
    description = "Actors following the Profile; each follower is a local Profile or a remote actor identified by its canonical actor URL (`follower.kind`, `follower.actor_url`; `follower.profile_id` only for local). NOT MOUNTED in production (contract only); the skeleton answers 501. Authorization: only the Profile's current owner.",
    params(
        ("profile_nanoid" = String, Path, description = "Profile nanoid"),
        ("limit" = Option<u32>, Query, description = "Pagination limit"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
    ),
    responses(
        (status = 200, description = "Followers list", body = ProfileFollowerListResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
        (status = 501, description = "Contract only: not implemented and not mounted on the production router"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn list_profile_followers() -> ErrorStatus {
    not_implemented("GET /api/v1/profiles/{profile_nanoid}/followers")
}

#[utoipa::path(
    post,
    path = "/api/v1/profiles/{profile_nanoid}/blocks",
    description = "Block one target (individual actor) as the Profile. NOT MOUNTED in production (contract only; block writes stay unmounted until D8R); the skeleton answers 501. \
Semantics: display suppression, detaches existing follow relations in both directions and rejects new follows/interactions in both directions. No ActivityPub Block is sent. Removing the block never restores detached relations. \
Authorization: only the Profile's current owner. Settings never propagate to other Profiles or Accounts.",
    params(("profile_nanoid" = String, Path, description = "Blocking Profile nanoid")),
    request_body = CreateProfileRelationRequest,
    responses(
        (status = 201, description = "Block recorded", body = ProfileRelationResponse),
        (status = 400, description = "Invalid target"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
        (status = 409, description = "Target is already blocked"),
        (status = 501, description = "Contract only: not implemented and not mounted on the production router"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn create_profile_block() -> ErrorStatus {
    not_implemented("POST /api/v1/profiles/{profile_nanoid}/blocks")
}

#[utoipa::path(
    delete,
    path = "/api/v1/profiles/{profile_nanoid}/blocks/{block_id}",
    description = "Remove an individual block. Detached follow relations are NOT restored and no restore list is kept. NOT MOUNTED in production (contract only); the skeleton answers 501. Authorization: only the Profile's current owner.",
    params(
        ("profile_nanoid" = String, Path, description = "Blocking Profile nanoid"),
        ("block_id" = String, Path, description = "Block setting nanoid"),
    ),
    responses(
        (status = 204, description = "Block removed"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile or block not found"),
        (status = 501, description = "Contract only: not implemented and not mounted on the production router"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn delete_profile_block() -> ErrorStatus {
    not_implemented("DELETE /api/v1/profiles/{profile_nanoid}/blocks/{block_id}")
}

#[utoipa::path(
    get,
    path = "/api/v1/profiles/{profile_nanoid}/blocks",
    description = "Individual blocks owned by the Profile. NOT MOUNTED in production (contract only); the skeleton answers 501. Authorization: only the Profile's current owner.",
    params(
        ("profile_nanoid" = String, Path, description = "Profile nanoid"),
        ("limit" = Option<u32>, Query, description = "Pagination limit"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
    ),
    responses(
        (status = 200, description = "Block list", body = ProfileRelationListResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
        (status = 501, description = "Contract only: not implemented and not mounted on the production router"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn list_profile_blocks() -> ErrorStatus {
    not_implemented("GET /api/v1/profiles/{profile_nanoid}/blocks")
}

#[utoipa::path(
    post,
    path = "/api/v1/profiles/{profile_nanoid}/mutes",
    description = "Mute one target (individual actor) as the Profile. NOT MOUNTED in production (contract only); the skeleton answers 501. \
Semantics: display suppression only; follow relations are kept and nothing is rejected. Authorization: only the Profile's current owner.",
    params(("profile_nanoid" = String, Path, description = "Muting Profile nanoid")),
    request_body = CreateProfileRelationRequest,
    responses(
        (status = 201, description = "Mute recorded", body = ProfileRelationResponse),
        (status = 400, description = "Invalid target"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
        (status = 409, description = "Target is already muted"),
        (status = 501, description = "Contract only: not implemented and not mounted on the production router"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn create_profile_mute() -> ErrorStatus {
    not_implemented("POST /api/v1/profiles/{profile_nanoid}/mutes")
}

#[utoipa::path(
    delete,
    path = "/api/v1/profiles/{profile_nanoid}/mutes/{mute_id}",
    description = "Remove an individual mute. NOT MOUNTED in production (contract only); the skeleton answers 501. Authorization: only the Profile's current owner.",
    params(
        ("profile_nanoid" = String, Path, description = "Muting Profile nanoid"),
        ("mute_id" = String, Path, description = "Mute setting nanoid"),
    ),
    responses(
        (status = 204, description = "Mute removed"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile or mute not found"),
        (status = 501, description = "Contract only: not implemented and not mounted on the production router"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn delete_profile_mute() -> ErrorStatus {
    not_implemented("DELETE /api/v1/profiles/{profile_nanoid}/mutes/{mute_id}")
}

#[utoipa::path(
    get,
    path = "/api/v1/profiles/{profile_nanoid}/mutes",
    description = "Individual mutes owned by the Profile. NOT MOUNTED in production (contract only); the skeleton answers 501. Authorization: only the Profile's current owner.",
    params(
        ("profile_nanoid" = String, Path, description = "Profile nanoid"),
        ("limit" = Option<u32>, Query, description = "Pagination limit"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
    ),
    responses(
        (status = 200, description = "Mute list", body = ProfileRelationListResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
        (status = 501, description = "Contract only: not implemented and not mounted on the production router"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn list_profile_mutes() -> ErrorStatus {
    not_implemented("GET /api/v1/profiles/{profile_nanoid}/mutes")
}

/// Mounts the contract skeleton handlers. Production code must NOT call this; the production
/// mount is owned by P4. It exists so tests can pin the 501 behaviour.
#[cfg_attr(not(test), allow(dead_code))]
pub trait ProfilePolicyContractRouter {
    /// Routes relative to `/api/v1`.
    fn route_profile_policy_contract(self) -> Self;
}

impl<S> ProfilePolicyContractRouter for Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn route_profile_policy_contract(self) -> Self {
        self.route("/profiles/{profile_nanoid}", get(get_profile_identity))
            .route(
                "/profiles/{profile_nanoid}/follows",
                post(create_profile_follow),
            )
            .route(
                "/profiles/{profile_nanoid}/follows/{follow_id}",
                delete(delete_profile_follow),
            )
            .route(
                "/profiles/{profile_nanoid}/following",
                get(list_profile_following),
            )
            .route(
                "/profiles/{profile_nanoid}/followers",
                get(list_profile_followers),
            )
            .route(
                "/profiles/{profile_nanoid}/blocks",
                get(list_profile_blocks).post(create_profile_block),
            )
            .route(
                "/profiles/{profile_nanoid}/blocks/{block_id}",
                delete(delete_profile_block),
            )
            .route(
                "/profiles/{profile_nanoid}/mutes",
                get(list_profile_mutes).post(create_profile_mute),
            )
            .route(
                "/profiles/{profile_nanoid}/mutes/{mute_id}",
                delete(delete_profile_mute),
            )
    }
}

#[cfg(test)]
#[path = "profile_policy_tests.rs"]
mod tests;
