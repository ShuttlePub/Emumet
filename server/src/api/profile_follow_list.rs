use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::dto::profile_inbound::{ProfileFollowDirection, ProfileFollowPageDto};
use application::service::profile_follow_list::ProfileFollowListUseCase;
use application::service::session_context::OrganizationContext;
use axum::extract::FromRef;
use kernel::KernelError;
use std::sync::Arc;

use super::resolve_auth_account_id;

#[derive(Clone)]
pub struct ProfileFollowListApi {
    module: Arc<AppModule>,
}

impl ProfileFollowListApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn list(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
        direction: ProfileFollowDirection,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> error_stack::Result<ProfileFollowPageDto, KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        self.module
            .list_profile_follows(
                &auth_account_id,
                org_context,
                profile_nanoid,
                direction,
                cursor,
                limit,
            )
            .await
    }
}

impl FromRef<AppModule> for ProfileFollowListApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
