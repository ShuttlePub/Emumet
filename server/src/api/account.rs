use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::dto::account::{AccountDetailDto, AccountDto, CreateAccountDto, UpdateAccountDto};
use application::dto::pagination::Pagination;
use application::service::account::{
    CreateAccountUseCase, DeactivateAccountUseCase, ReactivateAccountUseCase,
};
use application::service::account_detail::{GetAccountDetailUseCase, UpdateAccountDetailUseCase};
use application::service::profile::CreateOrganizationProfileUseCase;
use application::service::profile_username::SetProfileUsernameUseCase;
use application::service::session_context::OrganizationContext;
use axum::extract::FromRef;
use kernel::prelude::entity::AuthAccountId;
use kernel::KernelError;
use std::sync::Arc;

#[derive(Clone)]
pub struct AccountApi {
    module: Arc<AppModule>,
}

impl AccountApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn resolve_auth_account_id(
        &self,
        auth_info: OidcAuthInfo,
    ) -> error_stack::Result<AuthAccountId, KernelError> {
        resolve_auth_account_id(&self.module, auth_info).await
    }

    pub async fn get_account_details_by_ids(
        &self,
        auth_account_id: &AuthAccountId,
        ids: Vec<String>,
    ) -> error_stack::Result<Vec<AccountDetailDto>, KernelError> {
        self.module
            .get_account_details_by_ids(auth_account_id, ids)
            .await
    }

    pub async fn get_all_account_details(
        &self,
        auth_account_id: &AuthAccountId,
        pagination: Pagination<String>,
    ) -> error_stack::Result<Vec<AccountDetailDto>, KernelError> {
        self.module
            .get_all_account_details(auth_account_id, pagination)
            .await
    }

    pub async fn get_account_detail(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoid: String,
    ) -> error_stack::Result<AccountDetailDto, KernelError> {
        self.module
            .get_account_detail(auth_account_id, account_nanoid)
            .await
    }

    pub async fn create_account_with_profile_username(
        &self,
        auth_account_id: AuthAccountId,
        dto: CreateAccountDto,
        profile_username: String,
    ) -> error_stack::Result<AccountDto, KernelError> {
        self.module
            .create_account_with_profile_username(auth_account_id, dto, profile_username)
            .await
    }

    pub async fn set_profile_username(
        &self,
        auth_account_id: &AuthAccountId,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: String,
        username: String,
    ) -> error_stack::Result<application::dto::profile::ProfileDto, KernelError> {
        self.module
            .set_profile_username(auth_account_id, org_context, profile_nanoid, username)
            .await
    }

    pub async fn update_account_detail_in_context(
        &self,
        auth_account_id: &AuthAccountId,
        org_context: Option<&OrganizationContext>,
        dto: UpdateAccountDto,
    ) -> error_stack::Result<AccountDetailDto, KernelError> {
        self.module
            .update_account_detail_in_context(auth_account_id, org_context, dto)
            .await
    }

    pub async fn create_organization_profile(
        &self,
        org_context: OrganizationContext,
        dto: application::dto::profile::CreateProfileDto,
    ) -> error_stack::Result<application::dto::profile::ProfileDto, KernelError> {
        self.module
            .create_organization_profile(org_context, dto)
            .await
    }

    pub async fn deactivate_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
    ) -> error_stack::Result<(), KernelError> {
        self.module
            .deactivate_account(auth_account_id, account_id)
            .await
    }

    pub async fn reactivate_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
    ) -> error_stack::Result<(), KernelError> {
        self.module
            .reactivate_account(auth_account_id, account_id)
            .await
    }
}

impl FromRef<AppModule> for AccountApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
