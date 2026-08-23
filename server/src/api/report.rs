use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::service::report::{
    CreateReportUseCase, DismissReportUseCase, ListReportsUseCase, ReportList, ResolveReportUseCase,
};
use axum::extract::FromRef;
use kernel::interfaces::read_model::AccountReportProjection;
use kernel::prelude::entity::{AccountReportId, AuthAccountId, ReportCategory};
use kernel::KernelError;
use std::sync::Arc;

#[derive(Clone)]
pub struct ReportApi {
    module: Arc<AppModule>,
}

impl ReportApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn resolve_auth_account_id(
        &self,
        auth_info: OidcAuthInfo,
    ) -> error_stack::Result<AuthAccountId, KernelError> {
        resolve_auth_account_id(&self.module, auth_info).await
    }

    pub async fn create_report(
        &self,
        auth_account_id: &AuthAccountId,
        target_nanoid: String,
        category: ReportCategory,
        comment: Option<String>,
    ) -> error_stack::Result<AccountReportProjection, KernelError> {
        self.module
            .create_report(auth_account_id, target_nanoid, category, comment)
            .await
    }
}

impl FromRef<AppModule> for ReportApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}

#[derive(Clone)]
pub struct AdminReportApi {
    module: Arc<AppModule>,
}

impl AdminReportApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn resolve_auth_account_id(
        &self,
        auth_info: OidcAuthInfo,
    ) -> error_stack::Result<AuthAccountId, KernelError> {
        resolve_auth_account_id(&self.module, auth_info).await
    }

    pub async fn list_reports(
        &self,
        auth_account_id: &AuthAccountId,
    ) -> error_stack::Result<ReportList, KernelError> {
        self.module.list_reports(auth_account_id).await
    }

    pub async fn resolve_report(
        &self,
        auth_account_id: &AuthAccountId,
        report_id: AccountReportId,
        reason: String,
    ) -> error_stack::Result<(), KernelError> {
        self.module
            .resolve_report(auth_account_id, report_id, reason)
            .await
    }

    pub async fn dismiss_report(
        &self,
        auth_account_id: &AuthAccountId,
        report_id: AccountReportId,
        reason: String,
    ) -> error_stack::Result<(), KernelError> {
        self.module
            .dismiss_report(auth_account_id, report_id, reason)
            .await
    }
}

impl FromRef<AppModule> for AdminReportApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
