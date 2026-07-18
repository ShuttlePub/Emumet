mod handlers;

use super::delivery::deliver_activity_to_inbox;
use super::outbox::StoreOutboxActivityUseCase;
use crate::transfer::activitypub::InboxActivityDto;
use error_stack::Report;
use kernel::activitypub::Activity;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::crypto::{DependOnKeyEncryptor, DependOnPasswordProvider};
use kernel::interfaces::http_signing::DependOnHttpSigner;
use kernel::interfaces::repository::{
    DependOnFollowRepository, DependOnOutboxActivityRepository, DependOnRemoteAccountRepository,
    DependOnSigningKeyRepository,
};
use kernel::prelude::entity::AccountId;
use kernel::KernelError;
use std::future::Future;

pub trait InboxUseCase:
    'static
    + Sync
    + Send
    + DependOnFollowRepository
    + DependOnRemoteAccountRepository
    + DependOnSigningKeyRepository
    + DependOnHttpSigner
    + DependOnPasswordProvider
    + DependOnKeyEncryptor
    + DependOnPublicBaseUrl
    + DependOnOutboxActivityRepository
    + StoreOutboxActivityUseCase
{
    fn handle_inbox_activity(
        &self,
        dto: InboxActivityDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            match dto.activity.type_.as_str() {
                "Follow" => self.handle_follow_activity(dto).await,
                "Accept" => self.handle_accept_activity(dto).await,
                "Undo" if handlers::undo_object_is_follow(&dto.activity) => {
                    self.handle_undo_follow(dto).await
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

    fn handle_accept_activity(
        &self,
        dto: InboxActivityDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        handlers::handle_accept_activity(self, dto)
    }

    fn deliver_accept(
        &self,
        account_id: &AccountId,
        inbox_url: &Option<String>,
        accept: &Activity,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let inbox_url = inbox_url.as_deref().ok_or_else(|| {
                Report::new(KernelError::Rejected)
                    .attach_printable("Remote actor does not expose an inbox URL")
            })?;
            deliver_activity_to_inbox(
                self.database_connection(),
                self.signing_key_repository(),
                self.password_provider(),
                self.key_encryptor(),
                self.http_signer(),
                account_id,
                inbox_url,
                accept,
                "Accept",
            )
            .await
        }
    }
}

impl<T> InboxUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnFollowRepository
        + DependOnRemoteAccountRepository
        + DependOnSigningKeyRepository
        + DependOnHttpSigner
        + DependOnPasswordProvider
        + DependOnKeyEncryptor
        + DependOnPublicBaseUrl
        + DependOnOutboxActivityRepository
        + StoreOutboxActivityUseCase
{
}
