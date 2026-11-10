use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::service::registration_enforcement_probe::RegistrationEnforcementMonitorUseCase;
use application::service::registration_gate::{
    ChangeRegistrationGateUseCase, GetRegistrationGateUseCase, RegistrationGateNotificationUseCase,
    RegistrationGateNotifications, RegistrationGateView,
};
use axum::extract::FromRef;
use kernel::prelude::entity::{AuthAccountId, RegistrationGateEventId};
use kernel::KernelError;
use std::sync::Arc;

#[derive(Clone)]
pub struct AdminRegistrationGateApi {
    module: Arc<AppModule>,
}

impl AdminRegistrationGateApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn resolve_auth_account_id(
        &self,
        auth_info: OidcAuthInfo,
    ) -> error_stack::Result<AuthAccountId, KernelError> {
        resolve_auth_account_id(&self.module, auth_info).await
    }

    pub async fn get_registration_gate(
        &self,
        admin: &AuthAccountId,
    ) -> error_stack::Result<RegistrationGateView, KernelError> {
        self.module.get_registration_gate(admin).await?;
        self.module.refresh_registration_enforcement().await
    }

    pub async fn open_registration_gate(
        &self,
        admin: &AuthAccountId,
        revision: i64,
    ) -> error_stack::Result<RegistrationGateView, KernelError> {
        self.module.get_registration_gate(admin).await?;
        self.module.refresh_registration_enforcement().await?;
        self.module.open_registration_gate(admin, revision).await
    }

    pub async fn close_registration_gate(
        &self,
        admin: &AuthAccountId,
        revision: i64,
    ) -> error_stack::Result<RegistrationGateView, KernelError> {
        self.module.close_registration_gate(admin, revision).await
    }

    pub async fn enable_registration_gate_override(
        &self,
        admin: &AuthAccountId,
        revision: i64,
        warning_acknowledged: bool,
    ) -> error_stack::Result<RegistrationGateView, KernelError> {
        self.module.get_registration_gate(admin).await?;
        self.module.refresh_registration_enforcement().await?;
        self.module
            .enable_registration_gate_override(admin, revision, warning_acknowledged)
            .await
    }

    pub async fn revoke_registration_gate_override(
        &self,
        admin: &AuthAccountId,
        revision: i64,
    ) -> error_stack::Result<RegistrationGateView, KernelError> {
        self.module
            .revoke_registration_gate_override(admin, revision)
            .await
    }

    pub async fn list_notifications(
        &self,
        admin: &AuthAccountId,
        before: Option<RegistrationGateEventId>,
        limit: Option<i64>,
    ) -> error_stack::Result<RegistrationGateNotifications, KernelError> {
        self.module
            .list_registration_gate_notifications(admin, before, limit)
            .await
    }

    pub async fn count_unread_notifications(
        &self,
        admin: &AuthAccountId,
    ) -> error_stack::Result<i64, KernelError> {
        self.module
            .count_unread_registration_gate_notifications(admin)
            .await
    }

    pub async fn mark_notifications_read(
        &self,
        admin: &AuthAccountId,
        event_ids: Vec<RegistrationGateEventId>,
    ) -> error_stack::Result<i64, KernelError> {
        self.module
            .mark_registration_gate_notifications_read(admin, event_ids)
            .await
    }

    pub async fn mark_all_notifications_read(
        &self,
        admin: &AuthAccountId,
    ) -> error_stack::Result<i64, KernelError> {
        self.module
            .mark_all_registration_gate_notifications_read(admin)
            .await
    }
}

impl FromRef<AppModule> for AdminRegistrationGateApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
