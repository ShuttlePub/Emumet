use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::dto::profile::ProfileDto;
use application::dto::profile_relationship::{
    ProfileFollowDto, ProfileRelationDto, ProfileRelationItemDto,
};
use application::service::profile_relation_actor::ProfileRelationActorUseCase;
use application::service::profile_relation_list::{
    ProfileRelationListKind, ProfileRelationListUseCase, ProfileRelationPageDto,
};
use application::service::profile_relationship::ProfileRelationshipUseCase;
use application::service::session_context::OrganizationContext;
use axum::extract::FromRef;
use kernel::KernelError;
use std::sync::Arc;

/// Profile 単位の Follow / 個別 block・mute の送信側 use case への facade (P3o)。
/// P4 の切替で本番 root router に mount される (Profile identity・Follow・個別 block/mute)。
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

    pub async fn block(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
        target: &str,
    ) -> error_stack::Result<ProfileRelationItemDto, KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        let relation = self
            .module
            .block_actor(&auth_account_id, org_context, profile_nanoid, target)
            .await?;
        Ok(self.with_target_actor(relation).await)
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
    ) -> error_stack::Result<ProfileRelationItemDto, KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        let relation = self
            .module
            .mute_actor(&auth_account_id, org_context, profile_nanoid, target)
            .await?;
        Ok(self.with_target_actor(relation).await)
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

impl ProfileRelationshipApi {
    /// Attaches the display data of the new relation's target (from the Profile or the cache, no
    /// remote access) and schedules a background fetch for a remote target that is not cached yet.
    async fn with_target_actor(&self, relation: ProfileRelationDto) -> ProfileRelationItemDto {
        let (mut actors, refresh) = self
            .module
            .describe_relation_targets(std::slice::from_ref(&relation.target))
            .await;
        self.spawn_actor_refresh(refresh);
        ProfileRelationItemDto {
            target_actor: actors.remove(0),
            relation,
        }
    }

    fn spawn_actor_refresh(&self, actor_urls: Vec<String>) {
        if actor_urls.is_empty() {
            return;
        }
        let module = self.module.clone();
        tokio::spawn(async move {
            module.refresh_remote_actor_profiles(&actor_urls).await;
        });
    }

    pub async fn profile_identity(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
    ) -> error_stack::Result<ProfileDto, KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        self.module
            .get_profile_identity(&auth_account_id, org_context, profile_nanoid)
            .await
    }

    pub async fn list_relations(
        &self,
        auth_info: OidcAuthInfo,
        org_context: Option<&OrganizationContext>,
        profile_nanoid: &str,
        kind: ProfileRelationListKind,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> error_stack::Result<ProfileRelationPageDto, KernelError> {
        let auth_account_id = resolve_auth_account_id(&self.module, auth_info).await?;
        let page = self
            .module
            .list_profile_relations(
                &auth_account_id,
                org_context,
                profile_nanoid,
                kind,
                cursor,
                limit,
            )
            .await?;
        // The response never waits for a remote server: actors without (fresh) display data are
        // fetched in the background and show up on a later read.
        self.spawn_actor_refresh(page.refresh.clone());
        Ok(page)
    }
}

impl FromRef<AppModule> for ProfileRelationshipApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
