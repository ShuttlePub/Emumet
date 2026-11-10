use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::service::registration_gate_webhook::{
    RegistrationGateWebhookUseCase, WebhookAdminStatus,
};
use axum::extract::FromRef;
use kernel::interfaces::repository::WebhookTestOutcome;
use kernel::prelude::entity::{AuthAccountId, RegistrationGateEventId};
use kernel::KernelError;
use std::sync::Arc;

#[derive(Clone)]
pub struct AdminRegistrationGateWebhookApi {
    module: Arc<AppModule>,
}

impl AdminRegistrationGateWebhookApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn resolve_auth_account_id(
        &self,
        auth_info: OidcAuthInfo,
    ) -> error_stack::Result<AuthAccountId, KernelError> {
        resolve_auth_account_id(&self.module, auth_info).await
    }

    pub async fn get_webhook_status(
        &self,
        admin: &AuthAccountId,
    ) -> error_stack::Result<WebhookAdminStatus, KernelError> {
        self.module
            .get_registration_gate_webhook_status(admin)
            .await
    }

    pub async fn send_webhook_test(
        &self,
        admin: &AuthAccountId,
    ) -> error_stack::Result<WebhookTestOutcome, KernelError> {
        self.module.send_registration_gate_webhook_test(admin).await
    }

    pub async fn retry_failed_webhooks(
        &self,
        admin: &AuthAccountId,
        event_id: Option<RegistrationGateEventId>,
    ) -> error_stack::Result<u64, KernelError> {
        self.module
            .retry_failed_registration_gate_webhooks(admin, event_id)
            .await
    }
}

impl FromRef<AppModule> for AdminRegistrationGateWebhookApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
