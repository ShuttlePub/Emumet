//! Profile identity and relationship routes: the Profile identity read, Follow / unfollow and
//! individual block / mute (create, delete, list) as real handlers on the E8 contract paths.
//! The following / followers reads are in `route::profile_follow_list`; both mount functions are
//! combined on the production root router (the paths do not overlap).
//!
//! These routes are mounted on the production router by the P4 cutover (`root_router`); the
//! domain-controls routes by D8R (`route::profile_domain_control`). Authorization (current owner only), the
//! transaction that couples a relationship change with its delivery job, and block / mute
//! semantics live in `application::service::profile_relationship`.

use crate::api::profile_relationship::ProfileRelationshipApi;
use crate::auth::{AuthClaims, OidcAuthInfo, RequestOrganizationContext};
use crate::error::ErrorStatus;
use crate::schema::account::ProfileResponse;
use crate::schema::profile_policy::{
    CreateProfileFollowRequest, CreateProfileRelationRequest, ProfileFollowResponse,
    ProfileFollowState, ProfileRelationActor, ProfileRelationActorKind, ProfileRelationActorStatus,
    ProfileRelationKind, ProfileRelationListResponse, ProfileRelationResponse,
};
use application::dto::profile_relationship::{
    ProfileFollowDto, ProfileRelationItemDto, RelationActorDto, RelationActorKind,
    RelationActorStatus,
};
use application::service::profile_relation_list::ProfileRelationListKind;
use axum::extract::{FromRef, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use kernel::prelude::entity::ProfileFollowState as KernelFollowState;
use serde::Deserialize;

fn follow_response(dto: ProfileFollowDto) -> ProfileFollowResponse {
    ProfileFollowResponse {
        id: dto.nanoid,
        profile_id: dto.profile_nanoid,
        target: dto.target,
        // Only active generations are returned (a pending or an accepted one).
        state: match dto.state {
            KernelFollowState::Accepted => ProfileFollowState::Accepted,
            KernelFollowState::Pending | KernelFollowState::Ended => ProfileFollowState::Pending,
        },
        created_at: dto.created_at,
    }
}

fn actor_response(actor: RelationActorDto) -> ProfileRelationActor {
    ProfileRelationActor {
        kind: match actor.kind {
            RelationActorKind::Local => ProfileRelationActorKind::Local,
            RelationActorKind::Remote => ProfileRelationActorKind::Remote,
        },
        actor_url: actor.actor_url,
        profile_id: actor.profile_nanoid,
        status: match actor.status {
            RelationActorStatus::Resolved => ProfileRelationActorStatus::Resolved,
            RelationActorStatus::Pending => ProfileRelationActorStatus::Pending,
            RelationActorStatus::Unavailable => ProfileRelationActorStatus::Unavailable,
        },
        acct: actor.acct,
        preferred_username: actor.preferred_username,
        display_name: actor.display_name,
        icon_url: actor.icon_url,
    }
}

fn relation_response(
    item: ProfileRelationItemDto,
    kind: ProfileRelationKind,
) -> ProfileRelationResponse {
    let dto = item.relation;
    ProfileRelationResponse {
        id: dto.nanoid,
        profile_id: dto.profile_nanoid,
        kind,
        target: dto.target,
        created_at: dto.created_at,
        target_actor: actor_response(item.target_actor),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/profiles/{profile_nanoid}/follows",
    description = "Follow a target as the Profile (the Profile, not the Account, is the follower). \
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
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn follow_profile(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<ProfileRelationshipApi>,
    Path(profile_nanoid): Path<String>,
    Json(request): Json<CreateProfileFollowRequest>,
) -> Result<(StatusCode, Json<ProfileFollowResponse>), ErrorStatus> {
    let follow = api
        .follow(
            OidcAuthInfo::from(claims),
            org_context.as_ref(),
            &profile_nanoid,
            &request.target,
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok((StatusCode::CREATED, Json(follow_response(follow))))
}

#[utoipa::path(
    delete,
    path = "/api/v1/profiles/{profile_nanoid}/follows/{follow_id}",
    description = "Unfollow: remove a follow owned by the Profile. Authorization: only the Profile's current owner.",
    params(
        ("profile_nanoid" = String, Path, description = "Follower Profile nanoid"),
        ("follow_id" = String, Path, description = "Follow relation nanoid"),
    ),
    responses(
        (status = 204, description = "Follow removed"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile or follow not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn unfollow_profile(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<ProfileRelationshipApi>,
    Path((profile_nanoid, follow_id)): Path<(String, String)>,
) -> Result<StatusCode, ErrorStatus> {
    api.unfollow(
        OidcAuthInfo::from(claims),
        org_context.as_ref(),
        &profile_nanoid,
        &follow_id,
    )
    .await
    .map_err(ErrorStatus::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/profiles/{profile_nanoid}/blocks",
    description = "Block one target (individual actor) as the Profile. \
Semantics: display suppression, detaches existing follow relations in both directions and rejects new follows/interactions in both directions. In the same transaction the detached follows are announced to a remote actor with Undo(Follow) (your Follow) / Reject(Follow) (their Follow) and an ActivityPub `Block` (object = the blocked actor) is delivered to its inbox; a local target needs no delivery and a peer that cannot be resolved does not fail the block. (A domain block sends no ActivityPub Block.) Removing the block never restores detached relations. \
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
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn block_actor(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<ProfileRelationshipApi>,
    Path(profile_nanoid): Path<String>,
    Json(request): Json<CreateProfileRelationRequest>,
) -> Result<Response, ErrorStatus> {
    let (block, refresh) = api
        .block(
            OidcAuthInfo::from(claims),
            org_context.as_ref(),
            &profile_nanoid,
            &request.target,
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok(refresh.attach(
        (
            StatusCode::CREATED,
            Json(relation_response(block, ProfileRelationKind::Block)),
        )
            .into_response(),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/profiles/{profile_nanoid}/blocks/{block_id}",
    description = "Remove an individual block. Detached follow relations are NOT restored and no restore list is kept. A `Block` already delivered to a remote actor is taken back with an `Undo(Block)` that embeds it; a `Block` not yet sent is cancelled and no Undo is sent. Authorization: only the Profile's current owner.",
    params(
        ("profile_nanoid" = String, Path, description = "Blocking Profile nanoid"),
        ("block_id" = String, Path, description = "Block setting nanoid"),
    ),
    responses(
        (status = 204, description = "Block removed"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile or block not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn unblock_actor(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<ProfileRelationshipApi>,
    Path((profile_nanoid, block_id)): Path<(String, String)>,
) -> Result<StatusCode, ErrorStatus> {
    api.unblock(
        OidcAuthInfo::from(claims),
        org_context.as_ref(),
        &profile_nanoid,
        &block_id,
    )
    .await
    .map_err(ErrorStatus::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/profiles/{profile_nanoid}/mutes",
    description = "Mute one target (individual actor) as the Profile. \
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
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn mute_actor(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<ProfileRelationshipApi>,
    Path(profile_nanoid): Path<String>,
    Json(request): Json<CreateProfileRelationRequest>,
) -> Result<Response, ErrorStatus> {
    let (mute, refresh) = api
        .mute(
            OidcAuthInfo::from(claims),
            org_context.as_ref(),
            &profile_nanoid,
            &request.target,
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok(refresh.attach(
        (
            StatusCode::CREATED,
            Json(relation_response(mute, ProfileRelationKind::Mute)),
        )
            .into_response(),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/profiles/{profile_nanoid}/mutes/{mute_id}",
    description = "Remove an individual mute. Authorization: only the Profile's current owner.",
    params(
        ("profile_nanoid" = String, Path, description = "Muting Profile nanoid"),
        ("mute_id" = String, Path, description = "Mute setting nanoid"),
    ),
    responses(
        (status = 204, description = "Mute removed"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile or mute not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn unmute_actor(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<ProfileRelationshipApi>,
    Path((profile_nanoid, mute_id)): Path<(String, String)>,
) -> Result<StatusCode, ErrorStatus> {
    api.unmute(
        OidcAuthInfo::from(claims),
        org_context.as_ref(),
        &profile_nanoid,
        &mute_id,
    )
    .await
    .map_err(ErrorStatus::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub(crate) struct RelationListQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/profiles/{profile_nanoid}",
    description = "Profile identity read. \
Authorization: only the Profile's current owner (the owning personal Account, or an active member of the owning organization via X-Organization-Id). \
`username` is set for every Profile once the cutover has run (it is required at creation, and the cutover refuses to run while a Profile has none); it is immutable and there is deliberately no username update API.",
    params(("profile_nanoid" = String, Path, description = "Profile nanoid (external ID)")),
    responses(
        (status = 200, description = "Profile identity", body = ProfileResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn get_profile_identity(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<ProfileRelationshipApi>,
    Path(profile_nanoid): Path<String>,
) -> Result<Json<ProfileResponse>, ErrorStatus> {
    let profile = api
        .profile_identity(
            OidcAuthInfo::from(claims),
            org_context.as_ref(),
            &profile_nanoid,
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(profile.into()))
}

async fn list_relations(
    claims: AuthClaims,
    org_context: Option<application::service::session_context::OrganizationContext>,
    api: ProfileRelationshipApi,
    profile_nanoid: String,
    query: RelationListQuery,
    kind: ProfileRelationListKind,
) -> Result<Response, ErrorStatus> {
    let response_kind = match kind {
        ProfileRelationListKind::Block => ProfileRelationKind::Block,
        ProfileRelationListKind::Mute => ProfileRelationKind::Mute,
    };
    let (page, refresh) = api
        .list_relations(
            OidcAuthInfo::from(claims),
            org_context.as_ref(),
            &profile_nanoid,
            kind,
            query.cursor.as_deref(),
            query.limit,
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok(refresh.attach(
        Json(ProfileRelationListResponse {
            items: page
                .items
                .into_iter()
                .map(|item| relation_response(item, response_kind))
                .collect(),
            next_cursor: page.next_cursor,
        })
        .into_response(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/profiles/{profile_nanoid}/blocks",
    description = "Individual blocks owned by the Profile. Authorization: only the Profile's current owner. Each item carries `target_actor` (name, icon, acct) served from the Profile or a server-side cache; the list never contacts a remote server, so a remote target not yet fetched is `pending` (a background fetch is started) and a failed one `unavailable`: fall back to `target`.",
    params(
        ("profile_nanoid" = String, Path, description = "Profile nanoid"),
        ("limit" = Option<u32>, Query, description = "Pagination limit"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
    ),
    responses(
        (status = 200, description = "Block list", body = ProfileRelationListResponse),
        (status = 400, description = "Invalid cursor"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn list_profile_blocks(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<ProfileRelationshipApi>,
    Path(profile_nanoid): Path<String>,
    Query(query): Query<RelationListQuery>,
) -> Result<Response, ErrorStatus> {
    list_relations(
        claims,
        org_context,
        api,
        profile_nanoid,
        query,
        ProfileRelationListKind::Block,
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/profiles/{profile_nanoid}/mutes",
    description = "Individual mutes owned by the Profile. Authorization: only the Profile's current owner. Each item carries `target_actor` (name, icon, acct) served from the Profile or a server-side cache; the list never contacts a remote server, so a remote target not yet fetched is `pending` (a background fetch is started) and a failed one `unavailable`: fall back to `target`.",
    params(
        ("profile_nanoid" = String, Path, description = "Profile nanoid"),
        ("limit" = Option<u32>, Query, description = "Pagination limit"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
    ),
    responses(
        (status = 200, description = "Mute list", body = ProfileRelationListResponse),
        (status = 400, description = "Invalid cursor"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn list_profile_mutes(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<ProfileRelationshipApi>,
    Path(profile_nanoid): Path<String>,
    Query(query): Query<RelationListQuery>,
) -> Result<Response, ErrorStatus> {
    list_relations(
        claims,
        org_context,
        api,
        profile_nanoid,
        query,
        ProfileRelationListKind::Mute,
    )
    .await
}

/// Mounts the Profile identity and relationship routes on the production root router.
pub trait ProfileRelationshipRouter {
    /// Routes relative to `/api/v1`.
    fn route_profile_relationships(self) -> Self;
}

impl<S> ProfileRelationshipRouter for Router<S>
where
    S: Clone + Send + Sync + 'static,
    ProfileRelationshipApi: FromRef<S>,
{
    fn route_profile_relationships(self) -> Self {
        self.route("/profiles/{profile_nanoid}", get(get_profile_identity))
            .route("/profiles/{profile_nanoid}/follows", post(follow_profile))
            .route(
                "/profiles/{profile_nanoid}/follows/{follow_id}",
                delete(unfollow_profile),
            )
            .route(
                "/profiles/{profile_nanoid}/blocks",
                get(list_profile_blocks).post(block_actor),
            )
            .route(
                "/profiles/{profile_nanoid}/blocks/{block_id}",
                delete(unblock_actor),
            )
            .route(
                "/profiles/{profile_nanoid}/mutes",
                get(list_profile_mutes).post(mute_actor),
            )
            .route(
                "/profiles/{profile_nanoid}/mutes/{mute_id}",
                delete(unmute_actor),
            )
    }
}

#[cfg(test)]
#[path = "profile_relationship_tests.rs"]
mod tests;
