use crate::api::OrgAccountApi;
use crate::auth::{AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::schema::organization::{
    ChangeRoleRequest, CreateOrganizationRequest, InviteMemberRequest, MyOrganizationResponse,
    MyOrganizationsResponse, OrganizationMemberResponse, OrganizationMembersResponse,
    OrganizationResponse,
};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

async fn auth(
    api: &OrgAccountApi,
    claims: AuthClaims,
) -> Result<kernel::prelude::entity::AuthAccountId, ErrorStatus> {
    api.resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)
}

#[utoipa::path(post, path = "/api/v1/organizations", request_body = CreateOrganizationRequest, responses((status = 201, body = OrganizationResponse), (status = 400)), security(("bearer_auth" = [])), tag = "Organization")]
pub(crate) async fn create_organization(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<OrgAccountApi>,
    Json(request): Json<CreateOrganizationRequest>,
) -> Result<(StatusCode, Json<OrganizationResponse>), ErrorStatus> {
    if request.name.trim().is_empty() || request.name.len() > 100 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Organization name must be between 1 and 100 characters".to_string(),
        )
            .into());
    }
    let auth_id = auth(&api, claims).await?;
    let result = api
        .create(auth_id, request.into_dto())
        .await
        .map_err(ErrorStatus::from)?;
    Ok((StatusCode::CREATED, Json(result.into())))
}

#[utoipa::path(get, path = "/api/v1/me/organizations", responses((status = 200, body = MyOrganizationsResponse)), security(("bearer_auth" = [])), tag = "Organization")]
pub(crate) async fn list_my_organizations(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<OrgAccountApi>,
) -> Result<Json<MyOrganizationsResponse>, ErrorStatus> {
    let auth_id = auth(&api, claims).await?;
    let items = api.list_mine(&auth_id).await.map_err(ErrorStatus::from)?;
    Ok(Json(MyOrganizationsResponse {
        items: items
            .into_iter()
            .map(MyOrganizationResponse::from)
            .collect(),
    }))
}

#[utoipa::path(get, path = "/api/v1/organizations/{org}/members", params(("org" = String, Path)), responses((status = 200, body = OrganizationMembersResponse), (status = 403), (status = 404)), security(("bearer_auth" = [])), tag = "Organization")]
pub(crate) async fn list_organization_members(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<OrgAccountApi>,
    Path(org): Path<String>,
) -> Result<Json<OrganizationMembersResponse>, ErrorStatus> {
    let auth_id = auth(&api, claims).await?;
    let items = api
        .list_members(&auth_id, org)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(Json(OrganizationMembersResponse {
        items: items
            .into_iter()
            .map(OrganizationMemberResponse::from)
            .collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/organizations/{org}/invites", params(("org" = String, Path)), request_body = InviteMemberRequest, responses((status = 204), (status = 403), (status = 404), (status = 422)), security(("bearer_auth" = [])), tag = "Organization")]
pub(crate) async fn invite_member(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<OrgAccountApi>,
    Path(org): Path<String>,
    Json(request): Json<InviteMemberRequest>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_id = auth(&api, claims).await?;
    api.invite(auth_id, org, request.account_id, request.role.into())
        .await
        .map_err(ErrorStatus::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/organizations/{org}/invites/{account_id}/accept", params(("org" = String, Path), ("account_id" = String, Path)), responses((status = 204), (status = 403), (status = 404), (status = 422)), security(("bearer_auth" = [])), tag = "Organization")]
pub(crate) async fn accept_invite(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<OrgAccountApi>,
    Path((org, account_id)): Path<(String, String)>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_id = auth(&api, claims).await?;
    api.accept(auth_id, org, account_id)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(put, path = "/api/v1/organizations/{org}/members/{account_id}/role", params(("org" = String, Path), ("account_id" = String, Path)), request_body = ChangeRoleRequest, responses((status = 204), (status = 403), (status = 404), (status = 422)), security(("bearer_auth" = [])), tag = "Organization")]
pub(crate) async fn change_role(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<OrgAccountApi>,
    Path((org, account_id)): Path<(String, String)>,
    Json(request): Json<ChangeRoleRequest>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_id = auth(&api, claims).await?;
    api.change_role(auth_id, org, account_id, request.role.into())
        .await
        .map_err(ErrorStatus::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/api/v1/organizations/{org}/members/{account_id}", params(("org" = String, Path), ("account_id" = String, Path)), responses((status = 204), (status = 403), (status = 404), (status = 422)), security(("bearer_auth" = [])), tag = "Organization")]
pub(crate) async fn remove_member(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<OrgAccountApi>,
    Path((org, account_id)): Path<(String, String)>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_id = auth(&api, claims).await?;
    match api
        .leave(auth_id.clone(), org.clone(), account_id.clone())
        .await
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(error) if error.current_context() == &kernel::KernelError::PermissionDenied => {
            api.remove(auth_id, org, account_id)
                .await
                .map_err(ErrorStatus::from)?;
            Ok(StatusCode::NO_CONTENT)
        }
        Err(error) => Err(ErrorStatus::from(error)),
    }
}
