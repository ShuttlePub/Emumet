mod handlers;

use super::outbox::{DeliverOutboxActivityUseCase, StoreOutboxActivityUseCase};
use crate::dto::activitypub::InboxActivityDto;
use kernel::activitypub::Activity;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::crypto::{DependOnKeyEncryptor, DependOnPasswordProvider};
use kernel::interfaces::database::DependOnTransactionManager;
use kernel::interfaces::http_signing::DependOnHttpSigner;
use kernel::interfaces::repository::{
    DependOnBlockRepository, DependOnFollowRepository, DependOnOutboxActivityRepository,
    DependOnRemoteAccountRepository, DependOnSigningKeyRepository,
};
use kernel::prelude::entity::{AccountId, OutboxActivityId};
use kernel::KernelError;
use std::future::Future;

pub trait InboxUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnFollowRepository
    + DependOnBlockRepository
    + DependOnRemoteAccountRepository
    + DependOnSigningKeyRepository
    + DependOnHttpSigner
    + DependOnPasswordProvider
    + DependOnKeyEncryptor
    + DependOnPublicBaseUrl
    + DependOnOutboxActivityRepository
    + DependOnTransactionManager
    + StoreOutboxActivityUseCase
    + DeliverOutboxActivityUseCase
{
    fn handle_inbox_activity(
        &self,
        dto: InboxActivityDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            match dto.activity.type_.as_str() {
                "Follow" => self.handle_follow_activity(dto).await,
                "Accept" => self.handle_accept_activity(dto).await,
                "Block" => self.handle_block_activity(dto).await,
                "Undo" if handlers::undo_object_is_follow(&dto.activity) => {
                    self.handle_undo_follow(dto).await
                }
                "Undo" if handlers::undo_object_is_block(&dto.activity) => {
                    self.handle_undo_block_activity(dto).await
                }
                activity_type => {
                    tracing::info!(
                        activity_type,
                        "Ignoring unsupported ActivityPub inbox activity"
                    );
                    Ok(())
                }
            }
        }
    }

    fn handle_follow_activity(
        &self,
        dto: InboxActivityDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        handlers::handle_follow_activity(self, dto)
    }

    fn handle_undo_follow(
        &self,
        dto: InboxActivityDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        handlers::handle_undo_follow(self, dto)
    }

    fn handle_block_activity(
        &self,
        dto: InboxActivityDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        handlers::handle_block_activity(self, dto)
    }

    fn handle_undo_block_activity(
        &self,
        dto: InboxActivityDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        handlers::handle_undo_block_activity(self, dto)
    }

    fn handle_accept_activity(
        &self,
        dto: InboxActivityDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        handlers::handle_accept_activity(self, dto)
    }

    fn deliver_accept(
        &self,
        account_id: &AccountId,
        outbox_id: &OutboxActivityId,
        inbox_url: &str,
        accept: &Activity,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            self.deliver_outbox_activity(outbox_id, account_id, inbox_url, accept, "Accept")
                .await
        }
    }
}

impl<T> InboxUseCase for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnFollowRepository
        + DependOnBlockRepository
        + DependOnRemoteAccountRepository
        + DependOnSigningKeyRepository
        + DependOnHttpSigner
        + DependOnPasswordProvider
        + DependOnKeyEncryptor
        + DependOnPublicBaseUrl
        + DependOnOutboxActivityRepository
        + DependOnTransactionManager
        + StoreOutboxActivityUseCase
        + DeliverOutboxActivityUseCase
{
}
