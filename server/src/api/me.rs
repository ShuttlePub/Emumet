use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::service::session_context::{GetSessionContextUseCase, SessionContext};
use axum::extract::FromRef;
use kernel::prelude::entity::AuthAccountId;
use kernel::KernelError;
use std::sync::Arc;

#[derive(Clone)]
pub struct MeApi {
    module: Arc<AppModule>,
}

impl MeApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn resolve_auth_account_id(
        &self,
        auth_info: OidcAuthInfo,
    ) -> error_stack::Result<AuthAccountId, KernelError> {
        resolve_auth_account_id(&self.module, auth_info).await
    }

    pub async fn get_session_context(
        &self,
        auth_account_id: &AuthAccountId,
    ) -> error_stack::Result<SessionContext, KernelError> {
        self.module.get_session_context(auth_account_id).await
    }
}

impl FromRef<AppModule> for MeApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
