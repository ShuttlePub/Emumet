use crate::api::profile_link::ProfileLinkApi;
use crate::auth::{AuthClaims, OidcAuthInfo, RequestOrganizationContext};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::schema::profile_link::{
    ProfileLinkListResponse, ProfileLinkResponse, RegisterProfileLink,
};
use axum::extract::{rejection::JsonRejection, Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Extension, Json, Router};
fn link_error(e: error_stack::Report<kernel::KernelError>) -> ErrorStatus {
    // A failed bounded DB operation is retryable, never a successful unlink/HTTP 408.
    if matches!(e.current_context(), kernel::KernelError::Timeout) {
        ErrorStatus::StatusCode(StatusCode::SERVICE_UNAVAILABLE)
    } else {
        ErrorStatus::from(e)
    }
}
#[utoipa::path(post, path="/api/v1/profiles/{profile_nanoid}/links",
    request_body=RegisterProfileLink,
    params(("profile_nanoid"=String,Path)),
    responses((status=201,description="Home registered",body=ProfileLinkResponse),(status=200,description="Same active home",body=ProfileLinkResponse),
        (status=400,description="Invalid or non-allowed origin"),(status=401,description="Unauthenticated"),(status=403,description="Not current active owner"),(status=404,description="Profile absent"),(status=409,description="Different active home or concurrent owner change"),(status=422,description="Unsupported trust mode"),(status=503,description="Retryable database timeout")),
    security(("bearer_auth"=[])),tag="ProfileLinks")]
pub(crate) async fn register_profile_link(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org)): Extension<RequestOrganizationContext>,
    State(api): State<ProfileLinkApi>,
    Path(profile): Path<String>,
    request: Result<Json<RegisterProfileLink>, JsonRejection>,
) -> Result<(StatusCode, Json<ProfileLinkResponse>), ErrorStatus> {
    let Json(request) = request.map_err(|_| ErrorStatus::StatusCode(StatusCode::BAD_REQUEST))?;
    let (link, new) = api
        .register(
            OidcAuthInfo::from(claims),
            org.as_ref(),
            &profile,
            &request.origin,
            request.trust_mode,
        )
        .await
        .map_err(link_error)?;
    Ok((
        if new {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(link.into()),
    ))
}
#[utoipa::path(get,path="/api/v1/profiles/{profile_nanoid}/links",params(("profile_nanoid"=String,Path)),
    responses((status=200,description="Current-owner ledger",body=ProfileLinkListResponse),(status=401,description="Unauthenticated"),(status=403,description="Not current active owner"),(status=404,description="Profile absent"),(status=503,description="Retryable database timeout")),security(("bearer_auth"=[])),tag="ProfileLinks")]
pub(crate) async fn list_profile_links(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org)): Extension<RequestOrganizationContext>,
    State(api): State<ProfileLinkApi>,
    Path(profile): Path<String>,
) -> Result<Json<ProfileLinkListResponse>, ErrorStatus> {
    let links = api
        .list(OidcAuthInfo::from(claims), org.as_ref(), &profile)
        .await
        .map_err(link_error)?;
    Ok(Json(ProfileLinkListResponse {
        items: links.into_iter().map(Into::into).collect(),
    }))
}
#[utoipa::path(delete,path="/api/v1/profiles/{profile_nanoid}/links/{link_id}",params(("profile_nanoid"=String,Path),("link_id"=String,Path)),
    responses((status=204,description="Unlinked or already unlinked"),(status=401,description="Unauthenticated"),(status=403,description="Not current active owner"),(status=404,description="Profile or link absent"),(status=503,description="Retryable database timeout")),security(("bearer_auth"=[])),tag="ProfileLinks")]
pub(crate) async fn unlink_profile_link(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org)): Extension<RequestOrganizationContext>,
    State(api): State<ProfileLinkApi>,
    Path((profile, id)): Path<(String, String)>,
) -> Result<StatusCode, ErrorStatus> {
    api.unlink(OidcAuthInfo::from(claims), org.as_ref(), &profile, &id)
        .await
        .map_err(link_error)?;
    Ok(StatusCode::NO_CONTENT)
}
#[cfg(test)]
#[path = "profile_link_tests.rs"]
mod tests;

pub trait ProfileLinkRouter {
    fn route_profile_links(self) -> Self;
}
impl ProfileLinkRouter for Router<AppModule> {
    fn route_profile_links(self) -> Self {
        self.route(
            "/profiles/{profile_nanoid}/links",
            get(list_profile_links).post(register_profile_link),
        )
        .route(
            "/profiles/{profile_nanoid}/links/{link_id}",
            delete(unlink_profile_link),
        )
    }
}
