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
use axum::response::Response;
use kernel::KernelError;
use std::sync::Arc;

/// 応答を返した後に始める remote actor の表示情報の取得 (バックグラウンド、best-effort)。
///
/// 一覧・作成の応答は取得を待たない。取得は [`ActorRefresh::attach`] で応答本体に結び付け、HTTP サーバーが
/// 本体を最後まで取り出した後 (または本体を破棄した後) に初めて起動する
/// (`api::start_after_body` の「応答を返した後」の定義を参照)。
pub struct ActorRefresh {
    module: Arc<AppModule>,
    actor_urls: Vec<String>,
}

impl ActorRefresh {
    fn new(module: Arc<AppModule>, actor_urls: Vec<String>) -> Self {
        Self { module, actor_urls }
    }

    /// 取得するものがあれば、本体を取り出し終えた後に起動する形で `response` に結び付ける。
    /// 取得するものが無ければ `response` をそのまま返す。
    pub fn attach(self, response: Response) -> Response {
        let has_work = !self.actor_urls.is_empty();
        super::start_after_body::start_after_body_if(response, has_work, move || self.start())
    }

    /// 実行中の tokio runtime があるときだけ取得を spawn する (本体の Drop からも呼ばれるため、
    /// runtime の外で破棄されても panic しない)。
    fn start(self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            self.module
                .refresh_remote_actor_profiles(&self.actor_urls)
                .await;
        });
    }
}

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
    ) -> error_stack::Result<(ProfileRelationItemDto, ActorRefresh), KernelError> {
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
    ) -> error_stack::Result<(ProfileRelationItemDto, ActorRefresh), KernelError> {
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
    /// remote access) and returns the background fetch for a remote target that is not cached yet;
    /// the caller attaches it to the response so that it starts only after the HTTP server has taken the whole response body (or dropped it).
    async fn with_target_actor(
        &self,
        relation: ProfileRelationDto,
    ) -> (ProfileRelationItemDto, ActorRefresh) {
        let (mut actors, refresh) = self
            .module
            .describe_relation_targets(std::slice::from_ref(&relation.target))
            .await;
        (
            ProfileRelationItemDto {
                target_actor: actors.remove(0),
                relation,
            },
            ActorRefresh::new(self.module.clone(), refresh),
        )
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
    ) -> error_stack::Result<(ProfileRelationPageDto, ActorRefresh), KernelError> {
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
        // fetched in the background (after the response body has been taken) and show up on a later read.
        let refresh = ActorRefresh::new(self.module.clone(), page.refresh.clone());
        Ok((page, refresh))
    }
}

impl FromRef<AppModule> for ProfileRelationshipApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
