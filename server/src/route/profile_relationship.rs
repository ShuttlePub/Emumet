//! Profile relationship routes (P3o): Follow / unfollow and individual block / mute as real handlers on the E8
//! contract paths. The following / followers reads are P3i's (`route::profile_follow_list`); both mount
//! functions can be combined on one router (the paths do not overlap).
//!
//! The routes are intentionally NOT part of the production root router: [`ProfileRelationshipRouter`]
//! is only used by tests until the P4 cutover mounts them. The OpenAPI description of these paths
//! stays with the contract skeletons in `route::profile_policy` (unchanged by this unit). Authorization
//! (current owner only), the transaction that couples a relationship change with its delivery job,
//! and block / mute semantics live in `application::service::profile_relationship`.

use crate::api::profile_relationship::ProfileRelationshipApi;
use crate::auth::{AuthClaims, OidcAuthInfo, RequestOrganizationContext};
use crate::error::ErrorStatus;
use crate::schema::profile_policy::{
    CreateProfileFollowRequest, CreateProfileRelationRequest, ProfileFollowResponse,
    ProfileFollowState, ProfileRelationKind, ProfileRelationResponse,
};
use application::dto::profile_relationship::{ProfileFollowDto, ProfileRelationDto};
use axum::extract::{FromRef, Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, post};
use axum::{Extension, Json, Router};
use kernel::prelude::entity::ProfileFollowState as KernelFollowState;

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

/// Mounts the relationship routes. Production code must NOT call this: the production mount is
/// owned by P4 (block writes by D8R). Only tests use it.
#[cfg_attr(not(test), allow(dead_code))]
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
        self.route("/profiles/{profile_nanoid}/follows", post(follow_profile))
            .route(
                "/profiles/{profile_nanoid}/follows/{follow_id}",
                delete(unfollow_profile),
            )
            .route("/profiles/{profile_nanoid}/blocks", post(block_actor))
            .route(
                "/profiles/{profile_nanoid}/blocks/{block_id}",
                delete(unblock_actor),
            )
            .route("/profiles/{profile_nanoid}/mutes", post(mute_actor))
            .route(
                "/profiles/{profile_nanoid}/mutes/{mute_id}",
                delete(unmute_actor),
            )
    }
}

#[cfg(test)]
#[path = "profile_relationship_tests.rs"]
mod tests;
