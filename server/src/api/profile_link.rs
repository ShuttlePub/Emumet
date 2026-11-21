//! Route-facing facade: no raw executor/ports escape to HTTP handlers.
use crate::api::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::service::home_link::HomeLinkUseCase;
use application::service::session_context::OrganizationContext;
use axum::extract::FromRef;
use kernel::home_link::HomeLink;
use kernel::KernelError;
use std::sync::Arc;
#[derive(Clone)]
pub struct ProfileLinkApi {
    module: Arc<AppModule>,
}
impl ProfileLinkApi {
    pub async fn register(
        &self,
        auth: OidcAuthInfo,
        org: Option<&OrganizationContext>,
        profile: &str,
        origin: &str,
        mode: i16,
    ) -> error_stack::Result<(HomeLink, bool), KernelError> {
        let auth = resolve_auth_account_id(&self.module, auth).await?;
        self.module
            .register_home_link(&auth, org, profile, origin, mode)
            .await
    }
    pub async fn list(
        &self,
        auth: OidcAuthInfo,
        org: Option<&OrganizationContext>,
        profile: &str,
    ) -> error_stack::Result<Vec<HomeLink>, KernelError> {
        let auth = resolve_auth_account_id(&self.module, auth).await?;
        self.module.list_home_links(&auth, org, profile).await
    }
    pub async fn unlink(
        &self,
        auth: OidcAuthInfo,
        org: Option<&OrganizationContext>,
        profile: &str,
        id: &str,
    ) -> error_stack::Result<(), KernelError> {
        let auth = resolve_auth_account_id(&self.module, auth).await?;
        self.module.unlink_home_link(&auth, org, profile, id).await
    }
}
impl FromRef<AppModule> for ProfileLinkApi {
    fn from_ref(module: &AppModule) -> Self {
        Self {
            module: Arc::new(module.clone()),
        }
    }
}
