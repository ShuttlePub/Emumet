//! Profile followers / following reads (P3i).
//!
//! The handlers are real, but the routes are intentionally NOT part of the production root
//! router: [`ProfileFollowListRouter`] is only used by tests until the P4 mount. The response
//! shapes are the E8 contract (`ProfileFollowListResponse`, `ProfileFollowerListResponse`,
//! `ProfileActorRef`); authorization and reads live in
//! `application::service::profile_follow_list`.

use crate::api::profile_follow_list::ProfileFollowListApi;
use crate::auth::{AuthClaims, OidcAuthInfo, RequestOrganizationContext};
use crate::error::ErrorStatus;
use crate::schema::profile_policy::{
    ProfileActorKind, ProfileActorRef, ProfileFollowListResponse, ProfileFollowResponse,
    ProfileFollowState, ProfileFollowerListResponse, ProfileFollowerResponse,
};
use application::dto::profile_inbound::{
    ProfileActorRefDto, ProfileFollowDirection, ProfileFollowItemDto, ProfileFollowStateDto,
};
use axum::extract::{FromRef, Path, Query, State};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct FollowListQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

impl From<ProfileFollowStateDto> for ProfileFollowState {
    fn from(state: ProfileFollowStateDto) -> Self {
        match state {
            ProfileFollowStateDto::Pending => ProfileFollowState::Pending,
            ProfileFollowStateDto::Accepted => ProfileFollowState::Accepted,
        }
    }
}

impl From<ProfileActorRefDto> for ProfileActorRef {
    fn from(actor: ProfileActorRefDto) -> Self {
        match actor {
            ProfileActorRefDto::Local {
                actor_url,
                profile_nanoid,
            } => ProfileActorRef {
                kind: ProfileActorKind::Local,
                actor_url,
                profile_id: Some(profile_nanoid),
            },
            ProfileActorRefDto::Remote { actor_url } => ProfileActorRef {
                kind: ProfileActorKind::Remote,
                actor_url: actor_url.as_str().to_string(),
                profile_id: None,
            },
        }
    }
}

fn following_response(item: ProfileFollowItemDto) -> ProfileFollowResponse {
    ProfileFollowResponse {
        id: item.id,
        profile_id: item.profile_nanoid,
        target: item.counterpart.actor_url().to_string(),
        state: item.state.into(),
        created_at: item.created_at,
    }
}

fn follower_response(item: ProfileFollowItemDto) -> ProfileFollowerResponse {
    ProfileFollowerResponse {
        id: item.id,
        profile_id: item.profile_nanoid,
        follower: item.counterpart.into(),
        state: item.state.into(),
        created_at: item.created_at,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/profiles/{profile_nanoid}/following",
    description = "Actors followed by the Profile (pending and accepted follows; ended generations are not listed). `target` is the canonical actor URL (the Profile actor URL for a local target). NOT MOUNTED in production (implemented by P3i; the route is mounted only in tests until P4). \
Oldest first; `cursor` is the opaque `next_cursor` of the previous page, `limit` defaults to 20 (1..=100). Authorization: only the Profile's current owner; other Profiles' and Accounts' relations are never returned.",
    params(
        ("profile_nanoid" = String, Path, description = "Profile nanoid"),
        ("limit" = Option<u32>, Query, description = "Pagination limit"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
    ),
    responses(
        (status = 200, description = "Following list", body = ProfileFollowListResponse),
        (status = 400, description = "Invalid cursor"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn list_profile_following(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<ProfileFollowListApi>,
    Path(profile_nanoid): Path<String>,
    Query(query): Query<FollowListQuery>,
) -> Result<Json<ProfileFollowListResponse>, ErrorStatus> {
    let page = api
        .list(
            OidcAuthInfo::from(claims),
            org_context.as_ref(),
            &profile_nanoid,
            ProfileFollowDirection::Following,
            query.cursor.as_deref(),
            query.limit,
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(ProfileFollowListResponse {
        items: page.items.into_iter().map(following_response).collect(),
        next_cursor: page.next_cursor,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/profiles/{profile_nanoid}/followers",
    description = "Actors following the Profile (pending and accepted follows); each follower is a local Profile or a remote actor identified by its canonical actor URL (`follower.kind`, `follower.actor_url`; `follower.profile_id` only for local). NOT MOUNTED in production (implemented by P3i; the route is mounted only in tests until P4). \
Oldest first; `cursor` is the opaque `next_cursor` of the previous page, `limit` defaults to 20 (1..=100). Authorization: only the Profile's current owner; other Profiles' and Accounts' relations are never returned.",
    params(
        ("profile_nanoid" = String, Path, description = "Profile nanoid"),
        ("limit" = Option<u32>, Query, description = "Pagination limit"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
    ),
    responses(
        (status = 200, description = "Followers list", body = ProfileFollowerListResponse),
        (status = 400, description = "Invalid cursor"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn list_profile_followers(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<ProfileFollowListApi>,
    Path(profile_nanoid): Path<String>,
    Query(query): Query<FollowListQuery>,
) -> Result<Json<ProfileFollowerListResponse>, ErrorStatus> {
    let page = api
        .list(
            OidcAuthInfo::from(claims),
            org_context.as_ref(),
            &profile_nanoid,
            ProfileFollowDirection::Followers,
            query.cursor.as_deref(),
            query.limit,
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(ProfileFollowerListResponse {
        items: page.items.into_iter().map(follower_response).collect(),
        next_cursor: page.next_cursor,
    }))
}

/// Mounts the followers / following reads. Production code must NOT call this: the production
/// mount is owned by P4. Only tests use it.
#[cfg_attr(not(test), allow(dead_code))]
pub trait ProfileFollowListRouter {
    /// Routes relative to `/api/v1`.
    fn route_profile_follow_lists(self) -> Self;
}

impl<S> ProfileFollowListRouter for Router<S>
where
    S: Clone + Send + Sync + 'static,
    ProfileFollowListApi: FromRef<S>,
{
    fn route_profile_follow_lists(self) -> Self {
        self.route(
            "/profiles/{profile_nanoid}/following",
            get(list_profile_following),
        )
        .route(
            "/profiles/{profile_nanoid}/followers",
            get(list_profile_followers),
        )
    }
}

#[cfg(test)]
#[path = "profile_follow_list_tests.rs"]
mod tests;
