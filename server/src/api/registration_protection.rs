use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::service::registration_protection::{
    AdminRegistrationProtectionUseCase, AdminVerifyInput, AdminVerifyReport, ConfirmOutcome,
    IssueOutcome, IssueRequest, ProtectionReadinessView, RegistrationEmailChallengeUseCase,
};
use axum::extract::FromRef;
use kernel::prelude::entity::{AuthAccountId, Protection};
use kernel::KernelError;
use std::sync::Arc;

/// 未認証の登録前 email challenge (発行・確認)。proof の照合・消費は含めない (内部 use case)。
#[derive(Clone)]
pub struct RegistrationEmailApi {
    module: Arc<AppModule>,
}

impl RegistrationEmailApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn issue_challenge(
        &self,
        email: &str,
        flow_id: &str,
        source: &str,
    ) -> error_stack::Result<IssueOutcome, KernelError> {
        self.module
            .issue_registration_email_challenge(&IssueRequest {
                email,
                flow_id,
                source,
            })
            .await
    }

    pub async fn confirm_challenge(
        &self,
        challenge_id: &str,
        code: &str,
    ) -> error_stack::Result<ConfirmOutcome, KernelError> {
        self.module
            .confirm_registration_email_challenge(challenge_id, code)
            .await
    }
}

impl FromRef<AppModule> for RegistrationEmailApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}

#[derive(Clone)]
pub struct AdminRegistrationProtectionApi {
    module: Arc<AppModule>,
}

impl AdminRegistrationProtectionApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn resolve_auth_account_id(
        &self,
        auth_info: OidcAuthInfo,
    ) -> error_stack::Result<AuthAccountId, KernelError> {
        resolve_auth_account_id(&self.module, auth_info).await
    }

    pub async fn list(
        &self,
        admin: &AuthAccountId,
    ) -> error_stack::Result<[ProtectionReadinessView; 2], KernelError> {
        self.module.list_registration_protections(admin).await
    }

    pub async fn verify(
        &self,
        admin: &AuthAccountId,
        protection: Protection,
        input: AdminVerifyInput,
    ) -> error_stack::Result<AdminVerifyReport, KernelError> {
        self.module
            .verify_registration_protection(admin, protection, input)
            .await
    }
}

impl FromRef<AppModule> for AdminRegistrationProtectionApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
