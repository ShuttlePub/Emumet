use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::dto::profile_relationship::{
    ProfileFollowDto, ProfileFollowPage, ProfileRelationDto,
};
use application::service::profile_relationship::ProfileRelationshipUseCase;
use application::service::session_context::OrganizationContext;
use axum::extract::FromRef;
use kernel::KernelError;
use std::sync::Arc;

/// Profile 単位の Follow / 個別 block・mute の送信側 use case への facade (P3o)。
/// 本番の root router には mount されない (P4 が所有)。
#[derive(Clone)]
pub struct ProfileRelationshipApi {
    module: Arc<AppModule>,
}

impl ProfileRelationshipApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn follow(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
        target: &str,
    ) -> error_stack::Result<ProfileFollowDto, KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        self.module
            .follow_profile(&auth_account_id, org_context, profile_nanoid, target)
            .await
    }

    pub async fn unfollow(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
        follow_nanoid: &str,
    ) -> error_stack::Result<(), KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        self.module
            .unfollow_profile(&auth_account_id, org_context, profile_nanoid, follow_nanoid)
            .await
    }

    pub async fn list_following(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
        cursor: Option<&str>,
        limit: i64,
    ) -> error_stack::Result<ProfileFollowPage, KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        self.module
            .list_profile_following(&auth_account_id, org_context, profile_nanoid, cursor, limit)
            .await
    }

    pub async fn block(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
        target: &str,
    ) -> error_stack::Result<ProfileRelationDto, KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        self.module
            .block_actor(&auth_account_id, org_context, profile_nanoid, target)
            .await
    }

    pub async fn unblock(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
        block_nanoid: &str,
    ) -> error_stack::Result<(), KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        self.module
            .unblock_actor(&auth_account_id, org_context, profile_nanoid, block_nanoid)
            .await
    }

    pub async fn mute(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
        target: &str,
    ) -> error_stack::Result<ProfileRelationDto, KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        self.module
            .mute_actor(&auth_account_id, org_context, profile_nanoid, target)
            .await
    }

    pub async fn unmute(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
        mute_nanoid: &str,
    ) -> error_stack::Result<(), KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        self.module
            .unmute_actor(&auth_account_id, org_context, profile_nanoid, mute_nanoid)
            .await
    }
}

impl FromRef<AppModule> for ProfileRelationshipApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
