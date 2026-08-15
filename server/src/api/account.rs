use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::dto::account::{AccountDetailDto, AccountDto, CreateAccountDto, UpdateAccountDto};
use application::dto::activitypub::{
    FollowRelationDto, SendFollowDto, SendFollowResultDto, SendUndoFollowDto,
};
use application::dto::block_mute::{BlockAccountDto, MuteAccountDto, RelationDto};
use application::dto::pagination::Pagination;
use application::service::account::{CreateAccountUseCase, DeactivateAccountUseCase};
use application::service::account_detail::{GetAccountDetailUseCase, UpdateAccountDetailUseCase};
use application::service::activitypub::{
    GetFollowRelationsUseCase, SendFollowUseCase, SendUndoFollowUseCase,
};
use application::service::block::{BlockAccountUseCase, GetBlocksUseCase, UnblockAccountUseCase};
use application::service::mute::{GetMutesUseCase, MuteAccountUseCase, UnmuteAccountUseCase};
use axum::extract::FromRef;
use kernel::prelude::entity::AuthAccountId;
use kernel::KernelError;
use std::sync::Arc;

#[derive(Clone)]
pub struct AccountApi {
    module: Arc<AppModule>,
}

impl AccountApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn resolve_auth_account_id(
        &self,
        auth_info: OidcAuthInfo,
    ) -> error_stack::Result<AuthAccountId, KernelError> {
        resolve_auth_account_id(&self.module, auth_info).await
    }

    pub async fn get_account_details_by_ids(
        &self,
        auth_account_id: &AuthAccountId,
        ids: Vec<String>,
    ) -> error_stack::Result<Vec<AccountDetailDto>, KernelError> {
        self.module
            .get_account_details_by_ids(auth_account_id, ids)
            .await
    }

    pub async fn get_all_account_details(
        &self,
        auth_account_id: &AuthAccountId,
        pagination: Pagination<String>,
    ) -> error_stack::Result<Vec<AccountDetailDto>, KernelError> {
        self.module
            .get_all_account_details(auth_account_id, pagination)
            .await
    }

    pub async fn get_account_detail(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoid: String,
    ) -> error_stack::Result<AccountDetailDto, KernelError> {
        self.module
            .get_account_detail(auth_account_id, account_nanoid)
            .await
    }

    pub async fn create_account(
        &self,
        auth_account_id: AuthAccountId,
        dto: CreateAccountDto,
    ) -> error_stack::Result<AccountDto, KernelError> {
        self.module.create_account(auth_account_id, dto).await
    }

    pub async fn update_account_detail(
        &self,
        auth_account_id: &AuthAccountId,
        dto: UpdateAccountDto,
    ) -> error_stack::Result<AccountDetailDto, KernelError> {
        self.module
            .update_account_detail(auth_account_id, dto)
            .await
    }

    pub async fn deactivate_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
    ) -> error_stack::Result<(), KernelError> {
        self.module
            .deactivate_account(auth_account_id, account_id)
            .await
    }

    pub async fn block_account(
        &self,
        auth_account_id: AuthAccountId,
        dto: BlockAccountDto,
    ) -> error_stack::Result<RelationDto, KernelError> {
        self.module.block_account(auth_account_id, dto).await
    }

    pub async fn unblock_account(
        &self,
        auth_account_id: AuthAccountId,
        dto: BlockAccountDto,
    ) -> error_stack::Result<(), KernelError> {
        self.module.unblock_account(auth_account_id, dto).await
    }

    pub async fn get_blocks(
        &self,
        auth_account_id: AuthAccountId,
        account_nanoid: String,
    ) -> error_stack::Result<Vec<RelationDto>, KernelError> {
        self.module
            .get_blocks(auth_account_id, account_nanoid)
            .await
    }

    pub async fn mute_account(
        &self,
        auth_account_id: AuthAccountId,
        dto: MuteAccountDto,
    ) -> error_stack::Result<RelationDto, KernelError> {
        self.module.mute_account(auth_account_id, dto).await
    }

    pub async fn unmute_account(
        &self,
        auth_account_id: AuthAccountId,
        dto: MuteAccountDto,
    ) -> error_stack::Result<(), KernelError> {
        self.module.unmute_account(auth_account_id, dto).await
    }

    pub async fn get_mutes(
        &self,
        auth_account_id: AuthAccountId,
        account_nanoid: String,
    ) -> error_stack::Result<Vec<RelationDto>, KernelError> {
        self.module.get_mutes(auth_account_id, account_nanoid).await
    }

    pub async fn send_follow(
        &self,
        auth_account_id: AuthAccountId,
        dto: SendFollowDto,
    ) -> error_stack::Result<SendFollowResultDto, KernelError> {
        self.module.send_follow(auth_account_id, dto).await
    }

    pub async fn send_undo_follow(
        &self,
        auth_account_id: AuthAccountId,
        dto: SendUndoFollowDto,
    ) -> error_stack::Result<(), KernelError> {
        self.module.send_undo_follow(auth_account_id, dto).await
    }

    pub async fn get_followers(
        &self,
        auth_account_id: AuthAccountId,
        account_nanoid: String,
    ) -> error_stack::Result<Vec<FollowRelationDto>, KernelError> {
        self.module
            .get_followers(auth_account_id, account_nanoid)
            .await
    }

    pub async fn get_following(
        &self,
        auth_account_id: AuthAccountId,
        account_nanoid: String,
    ) -> error_stack::Result<Vec<FollowRelationDto>, KernelError> {
        self.module
            .get_following(auth_account_id, account_nanoid)
            .await
    }
}

impl FromRef<AppModule> for AccountApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
