//! Profile domain-controls routes (D8P): owner-only storage of exact-match domain mute/block.
//!
//! The handlers are real, but the routes are intentionally NOT part of the production root
//! router: [`DomainControlRouter`] is only used by tests until the production mount (P4, block
//! writes after D8R). Authorization, normalization and revision control live in
//! `application::service::domain_control`.

use crate::api::domain_control::DomainControlApi;
use crate::auth::{AuthClaims, OidcAuthInfo, RequestOrganizationContext};
use crate::error::ErrorStatus;
use crate::schema::profile_policy::{
    DomainControlListResponse, DomainControlMode, DomainControlResponse, PutDomainControlRequest,
};
use application::dto::domain_control::DomainControlDto;
use axum::extract::{FromRef, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router};
use kernel::prelude::entity::DomainControlMode as KernelDomainControlMode;
use serde::Deserialize;
use utoipa::IntoParams;

impl From<DomainControlMode> for KernelDomainControlMode {
    fn from(mode: DomainControlMode) -> Self {
        match mode {
            DomainControlMode::Mute => KernelDomainControlMode::Mute,
            DomainControlMode::Block => KernelDomainControlMode::Block,
        }
    }
}

impl From<KernelDomainControlMode> for DomainControlMode {
    fn from(mode: KernelDomainControlMode) -> Self {
        match mode {
            KernelDomainControlMode::Mute => DomainControlMode::Mute,
            KernelDomainControlMode::Block => DomainControlMode::Block,
        }
    }
}

impl From<DomainControlDto> for DomainControlResponse {
    fn from(dto: DomainControlDto) -> Self {
        Self {
            profile_id: dto.profile_nanoid,
            domain: dto.domain,
            mode: dto.mode.into(),
            revision: dto.revision,
            created_at: dto.created_at,
            updated_at: dto.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub(crate) struct DeleteDomainControlQuery {
    /// Current revision of the control being removed (required).
    expected_revision: u64,
}

#[utoipa::path(
    get,
    path = "/api/v1/profiles/{profile_nanoid}/domain-controls",
    description = "List the Profile's domain mute/block controls (ordered by domain). NOT MOUNTED in production (implemented by D8P; the route is mounted only in tests until P4/D8R). \
Domains match exactly (`example.com` does not cover `sub.example.com`). Controls are per Profile: they never apply to other Profiles of the same Account or to other Accounts, and are unrelated to server-wide HostModeration. \
Authorization: only the Profile's current owner (after a Profile transfer, the new owner; the controls stay attached to the Profile).",
    params(("profile_nanoid" = String, Path, description = "Profile nanoid")),
    responses(
        (status = 200, description = "Domain controls", body = DomainControlListResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn list_domain_controls(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<DomainControlApi>,
    Path(profile_nanoid): Path<String>,
) -> Result<Json<DomainControlListResponse>, ErrorStatus> {
    let items = api
        .list(
            OidcAuthInfo::from(claims),
            org_context.as_ref(),
            &profile_nanoid,
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(DomainControlListResponse {
        items: items.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/profiles/{profile_nanoid}/domain-controls/{domain}",
    description = "Read the control for one exact domain. NOT MOUNTED in production (implemented by D8P; the route is mounted only in tests until P4/D8R). \
The `domain` is normalized before lookup (case, trailing dot, IDNA): `Example.COM.` and `example.com` address the same control. Authorization: only the Profile's current owner.",
    params(
        ("profile_nanoid" = String, Path, description = "Profile nanoid"),
        ("domain" = String, Path, description = "Exact host name. Normalized: lowercase, one trailing dot removed, IDNA A-label (Punycode). No scheme, port, userinfo, path, wildcard or IP address (422)."),
    ),
    responses(
        (status = 200, description = "Domain control", body = DomainControlResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile or domain control not found"),
        (status = 422, description = "Malformed domain"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn get_domain_control(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<DomainControlApi>,
    Path((profile_nanoid, domain)): Path<(String, String)>,
) -> Result<Json<DomainControlResponse>, ErrorStatus> {
    let control = api
        .get(
            OidcAuthInfo::from(claims),
            org_context.as_ref(),
            &profile_nanoid,
            &domain,
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(control.into()))
}

#[utoipa::path(
    put,
    path = "/api/v1/profiles/{profile_nanoid}/domain-controls/{domain}",
    description = "Create or replace the mute/block control for one exact domain. NOT MOUNTED in production (implemented by D8P; the route is mounted only in tests until P4/D8R, block writes stay unmounted until D8R). \
Revision: `expected_revision` omitted creates (409 if the control already exists); `expected_revision` = current revision replaces (409 on mismatch, including a control deleted meanwhile). Every successful write increments the revision, which never repeats for a Profile x domain even across delete and re-create. Concurrent writers: exactly one succeeds, the others get 409. \
mute = display suppression only. block = display suppression, bidirectional follow detachment and rejection of new follows; no ActivityPub Block is sent and removing the block does not restore relations (D8P only stores the setting; applying it is D8G/D8R). \
Authorization: only the Profile's current owner.",
    params(
        ("profile_nanoid" = String, Path, description = "Profile nanoid"),
        ("domain" = String, Path, description = "Exact host name. Normalized: lowercase, one trailing dot removed, IDNA A-label (Punycode). No scheme, port, userinfo, path, wildcard or IP address (422)."),
    ),
    request_body = PutDomainControlRequest,
    responses(
        (status = 200, description = "Domain control created or replaced", body = DomainControlResponse),
        (status = 400, description = "Malformed JSON body"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile not found"),
        (status = 409, description = "Revision conflict (expected_revision does not match the current state)"),
        (status = 422, description = "Malformed domain, or request body is invalid (for example an unknown mode)"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn put_domain_control(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<DomainControlApi>,
    Path((profile_nanoid, domain)): Path<(String, String)>,
    Json(request): Json<PutDomainControlRequest>,
) -> Result<Json<DomainControlResponse>, ErrorStatus> {
    let control = api
        .put(
            OidcAuthInfo::from(claims),
            org_context.as_ref(),
            &profile_nanoid,
            &domain,
            request.mode.into(),
            request.expected_revision,
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(control.into()))
}

#[utoipa::path(
    delete,
    path = "/api/v1/profiles/{profile_nanoid}/domain-controls/{domain}",
    description = "Remove the control for one exact domain. Detached follow relations are NOT restored and no restore list is kept. NOT MOUNTED in production (implemented by D8P; the route is mounted only in tests until P4/D8R). \
`expected_revision` is required: 409 if it does not match the current revision. Authorization: only the Profile's current owner.",
    params(
        ("profile_nanoid" = String, Path, description = "Profile nanoid"),
        ("domain" = String, Path, description = "Exact host name. Normalized: lowercase, one trailing dot removed, IDNA A-label (Punycode). No scheme, port, userinfo, path, wildcard or IP address (422)."),
        DeleteDomainControlQuery,
    ),
    responses(
        (status = 204, description = "Domain control removed"),
        (status = 400, description = "Missing or invalid expected_revision"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Caller is not the current owner of the Profile"),
        (status = 404, description = "Profile or domain control not found"),
        (status = 409, description = "Revision conflict"),
        (status = 422, description = "Malformed domain"),
    ),
    security(("bearer_auth" = [])),
    tag = "ProfilePolicy",
)]
pub(crate) async fn delete_domain_control(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<DomainControlApi>,
    Path((profile_nanoid, domain)): Path<(String, String)>,
    Query(query): Query<DeleteDomainControlQuery>,
) -> Result<StatusCode, ErrorStatus> {
    api.delete(
        OidcAuthInfo::from(claims),
        org_context.as_ref(),
        &profile_nanoid,
        &domain,
        query.expected_revision,
    )
    .await
    .map_err(ErrorStatus::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[path = "profile_domain_control_tests.rs"]
mod tests;

/// Mounts the domain-controls routes. Production code must NOT call this: the production mount
/// is owned by P4 (block writes by D8R). Only tests use it.
#[cfg_attr(not(test), allow(dead_code))]
pub trait DomainControlRouter {
    /// Routes relative to `/api/v1`.
    fn route_domain_controls(self) -> Self;
}

impl<S> DomainControlRouter for Router<S>
where
    S: Clone + Send + Sync + 'static,
    DomainControlApi: FromRef<S>,
{
    fn route_domain_controls(self) -> Self {
        self.route(
            "/profiles/{profile_nanoid}/domain-controls",
            get(list_domain_controls),
        )
        .route(
            "/profiles/{profile_nanoid}/domain-controls/{domain}",
            get(get_domain_control)
                .put(put_domain_control)
                .delete(delete_domain_control),
        )
    }
}
