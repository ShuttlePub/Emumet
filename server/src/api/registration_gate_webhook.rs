use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::service::registration_gate_webhook::{
    CreateWebhookEndpointInput, RegistrationGateWebhookUseCase, UpdateWebhookEndpointInput,
    WebhookEndpointList,
};
use axum::extract::FromRef;
use kernel::interfaces::repository::WebhookTestOutcome;
use kernel::prelude::entity::{
    AuthAccountId, RegistrationGateEventId, WebhookEndpointId, WebhookEndpointView,
    WebhookFailedPage, WebhookFailureCursor,
};
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

    pub async fn list_endpoints(
        &self,
        admin: &AuthAccountId,
    ) -> error_stack::Result<WebhookEndpointList, KernelError> {
        self.module
            .list_registration_gate_webhook_endpoints(admin)
            .await
    }

    pub async fn get_endpoint(
        &self,
        admin: &AuthAccountId,
        id: WebhookEndpointId,
    ) -> error_stack::Result<WebhookEndpointView, KernelError> {
        self.module
            .get_registration_gate_webhook_endpoint(admin, id)
            .await
    }

    pub async fn create_endpoint(
        &self,
        admin: &AuthAccountId,
        input: CreateWebhookEndpointInput,
    ) -> error_stack::Result<WebhookEndpointView, KernelError> {
        self.module
            .create_registration_gate_webhook_endpoint(admin, input)
            .await
    }

    pub async fn update_endpoint(
        &self,
        admin: &AuthAccountId,
        id: WebhookEndpointId,
        input: UpdateWebhookEndpointInput,
    ) -> error_stack::Result<WebhookEndpointView, KernelError> {
        self.module
            .update_registration_gate_webhook_endpoint(admin, id, input)
            .await
    }

    pub async fn delete_endpoint(
        &self,
        admin: &AuthAccountId,
        id: WebhookEndpointId,
        revision: i64,
    ) -> error_stack::Result<(), KernelError> {
        self.module
            .delete_registration_gate_webhook_endpoint(admin, id, revision)
            .await
    }

    pub async fn send_test(
        &self,
        admin: &AuthAccountId,
        id: WebhookEndpointId,
    ) -> error_stack::Result<WebhookTestOutcome, KernelError> {
        self.module
            .send_registration_gate_webhook_test(admin, id)
            .await
    }

    pub async fn retry_failed(
        &self,
        admin: &AuthAccountId,
        endpoint_id: Option<WebhookEndpointId>,
        event_id: Option<RegistrationGateEventId>,
    ) -> error_stack::Result<u64, KernelError> {
        self.module
            .retry_failed_registration_gate_webhooks(admin, endpoint_id, event_id)
            .await
    }

    pub async fn list_failed(
        &self,
        admin: &AuthAccountId,
        endpoint_id: Option<WebhookEndpointId>,
        before: Option<WebhookFailureCursor>,
        limit: Option<i64>,
    ) -> error_stack::Result<WebhookFailedPage, KernelError> {
        self.module
            .list_failed_registration_gate_webhook_deliveries(admin, endpoint_id, before, limit)
            .await
    }
}

impl FromRef<AppModule> for AdminRegistrationGateWebhookApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
