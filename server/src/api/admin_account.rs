use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::service::account::{
    AssignInstanceRoleUseCase, BanAccountUseCase, RevokeInstanceRoleUseCase, SuspendAccountUseCase,
    UnsuspendAccountUseCase,
};
use axum::extract::FromRef;
use kernel::interfaces::permission::InstanceRole;
use kernel::prelude::entity::AuthAccountId;
use kernel::KernelError;
use std::sync::Arc;

#[derive(Clone)]
pub struct AdminAccountApi {
    module: Arc<AppModule>,
}

impl AdminAccountApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn resolve_auth_account_id(
        &self,
        auth_info: OidcAuthInfo,
    ) -> error_stack::Result<AuthAccountId, KernelError> {
        resolve_auth_account_id(&self.module, auth_info).await
    }

    pub async fn suspend_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
        reason: String,
        expires_at: Option<time::OffsetDateTime>,
    ) -> error_stack::Result<(), KernelError> {
        self.module
            .suspend_account(auth_account_id, account_id, reason, expires_at)
            .await
    }

    pub async fn unsuspend_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
    ) -> error_stack::Result<(), KernelError> {
        self.module
            .unsuspend_account(auth_account_id, account_id)
            .await
    }

    pub async fn ban_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
        reason: String,
    ) -> error_stack::Result<(), KernelError> {
        self.module
            .ban_account(auth_account_id, account_id, reason)
            .await
    }

    pub async fn assign_instance_role(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
        role: InstanceRole,
    ) -> error_stack::Result<(), KernelError> {
        self.module
            .assign_instance_role(auth_account_id, account_id, role)
            .await
    }

    pub async fn revoke_instance_role(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
        role: InstanceRole,
    ) -> error_stack::Result<(), KernelError> {
        self.module
            .revoke_instance_role(auth_account_id, account_id, role)
            .await
    }
}

impl FromRef<AppModule> for AdminAccountApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
