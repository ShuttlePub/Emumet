//! Profile identity and relationship routes: the Profile identity read, Follow / unfollow and
//! individual block / mute (create, delete, list) as real handlers on the E8 contract paths.
//! The following / followers reads are in `route::profile_follow_list`; both mount functions are
//! combined on the production root router (the paths do not overlap).
//!
//! These routes are mounted on the production router by the P4 cutover (`root_router`). The
//! domain-controls routes are NOT (they wait for D8R). Authorization (current owner only), the
//! transaction that couples a relationship change with its delivery job, and block / mute
//! semantics live in `application::service::profile_relationship`.

use crate::api::profile_relationship::ProfileRelationshipApi;
use crate::auth::{AuthClaims, OidcAuthInfo, RequestOrganizationContext};
use crate::error::ErrorStatus;
use crate::schema::account::ProfileResponse;
use crate::schema::profile_policy::{
    CreateProfileFollowRequest, CreateProfileRelationRequest, ProfileFollowResponse,
    ProfileFollowState, ProfileRelationKind, ProfileRelationListResponse, ProfileRelationResponse,
};
use application::dto::profile_relationship::{ProfileFollowDto, ProfileRelationDto};
use application::service::profile_relation_list::ProfileRelationListKind;
use axum::extract::{FromRef, Path, Query, State};
use axum::http::StatusCode;
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

fn relation_response(
    dto: ProfileRelationDto,
    kind: ProfileRelationKind,
) -> ProfileRelationResponse {
    ProfileRelationResponse {
        id: dto.nanoid,
        profile_id: dto.profile_nanoid,
        kind,
        target: dto.target,
        created_at: dto.created_at,
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
) -> Result<(StatusCode, Json<ProfileRelationResponse>), ErrorStatus> {
    let block = api
        .block(
            OidcAuthInfo::from(claims),
            org_context.as_ref(),
            &profile_nanoid,
            &request.target,
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok((
        StatusCode::CREATED,
        Json(relation_response(block, ProfileRelationKind::Block)),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/profiles/{profile_nanoid}/blocks/{block_id}",
    description = "Remove an individual block. Detached follow relations are NOT restored and no restore list is kept. Authorization: only the Profile's current owner.",
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
) -> Result<(StatusCode, Json<ProfileRelationResponse>), ErrorStatus> {
    let mute = api
        .mute(
            OidcAuthInfo::from(claims),
            org_context.as_ref(),
            &profile_nanoid,
            &request.target,
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok((
        StatusCode::CREATED,
        Json(relation_response(mute, ProfileRelationKind::Mute)),
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
) -> Result<Json<ProfileRelationListResponse>, ErrorStatus> {
    let response_kind = match kind {
        ProfileRelationListKind::Block => ProfileRelationKind::Block,
        ProfileRelationListKind::Mute => ProfileRelationKind::Mute,
    };
    let page = api
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
    Ok(Json(ProfileRelationListResponse {
        items: page
            .items
            .into_iter()
            .map(|item| relation_response(item, response_kind))
            .collect(),
        next_cursor: page.next_cursor,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/profiles/{profile_nanoid}/blocks",
    description = "Individual blocks owned by the Profile. Authorization: only the Profile's current owner.",
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
) -> Result<Json<ProfileRelationListResponse>, ErrorStatus> {
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
    description = "Individual mutes owned by the Profile. Authorization: only the Profile's current owner.",
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
) -> Result<Json<ProfileRelationListResponse>, ErrorStatus> {
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
