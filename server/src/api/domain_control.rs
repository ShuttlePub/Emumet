use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::dto::domain_control::DomainControlDto;
use application::service::domain_control::DomainControlUseCase;
use application::service::session_context::OrganizationContext;
use axum::extract::FromRef;
use kernel::prelude::entity::DomainControlMode;
use kernel::KernelError;
use std::sync::Arc;

#[derive(Clone)]
pub struct DomainControlApi {
    module: Arc<AppModule>,
}

impl DomainControlApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn list(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
    ) -> error_stack::Result<Vec<DomainControlDto>, KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        self.module
            .list_domain_controls(&auth_account_id, org_context, profile_nanoid)
            .await
    }

    pub async fn get(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
        domain: &str,
    ) -> error_stack::Result<DomainControlDto, KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        self.module
            .get_domain_control(&auth_account_id, org_context, profile_nanoid, domain)
            .await
    }

    pub async fn put(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
        domain: &str,
        mode: DomainControlMode,
        expected_revision: Option<u64>,
    ) -> error_stack::Result<DomainControlDto, KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        self.module
            .put_domain_control(
                &auth_account_id,
                org_context,
                profile_nanoid,
                domain,
                mode,
                expected_revision,
            )
            .await
    }

    pub async fn delete(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
        domain: &str,
        expected_revision: u64,
    ) -> error_stack::Result<(), KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        self.module
            .delete_domain_control(
                &auth_account_id,
                org_context,
                profile_nanoid,
                domain,
                expected_revision,
            )
            .await
    }
}

impl FromRef<AppModule> for DomainControlApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
