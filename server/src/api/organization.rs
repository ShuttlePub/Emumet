use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::dto::organization::{
    CreateOrganizationDto, MyOrganizationDto, OrganizationMemberDto, OrganizationSummaryDto,
};
use application::service::organization::{
    AcceptInviteUseCase, ChangeRoleUseCase, CreateOrganizationUseCase, InviteMemberUseCase,
    LeaveOrganizationUseCase, ListMyOrganizationsUseCase, ListOrganizationMembersUseCase,
    RemoveMemberUseCase,
};
use axum::extract::FromRef;
use kernel::prelude::entity::{AuthAccountId, OrgRole};
use kernel::KernelError;
use std::sync::Arc;

#[derive(Clone)]
pub struct OrgAccountApi {
    module: Arc<AppModule>,
}

impl OrgAccountApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }
    pub async fn resolve_auth_account_id(
        &self,
        info: OidcAuthInfo,
    ) -> error_stack::Result<AuthAccountId, KernelError> {
        resolve_auth_account_id(&self.module, info).await
    }
    pub async fn create(
        &self,
        auth: AuthAccountId,
        dto: CreateOrganizationDto,
    ) -> error_stack::Result<OrganizationSummaryDto, KernelError> {
        self.module.create_organization(auth, dto).await
    }
    pub async fn list_mine(
        &self,
        auth: &AuthAccountId,
    ) -> error_stack::Result<Vec<MyOrganizationDto>, KernelError> {
        self.module.list_my_organizations(auth).await
    }
    pub async fn list_members(
        &self,
        auth: &AuthAccountId,
        org: String,
    ) -> error_stack::Result<Vec<OrganizationMemberDto>, KernelError> {
        self.module.list_organization_members(auth, org).await
    }
    pub async fn invite(
        &self,
        auth: AuthAccountId,
        org: String,
        target: String,
        role: OrgRole,
    ) -> error_stack::Result<(), KernelError> {
        self.module.invite_member(auth, org, target, role).await
    }
    pub async fn accept(
        &self,
        auth: AuthAccountId,
        org: String,
        actor: String,
    ) -> error_stack::Result<(), KernelError> {
        self.module.accept_invite(auth, org, actor).await
    }
    pub async fn change_role(
        &self,
        auth: AuthAccountId,
        org: String,
        member: String,
        role: OrgRole,
    ) -> error_stack::Result<(), KernelError> {
        self.module.change_role(auth, org, member, role).await
    }
    pub async fn remove(
        &self,
        auth: AuthAccountId,
        org: String,
        member: String,
    ) -> error_stack::Result<(), KernelError> {
        self.module.remove_member(auth, org, member).await
    }
    pub async fn leave(
        &self,
        auth: AuthAccountId,
        org: String,
        actor: String,
    ) -> error_stack::Result<(), KernelError> {
        self.module.leave_organization(auth, org, actor).await
    }
}

impl FromRef<AppModule> for OrgAccountApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
