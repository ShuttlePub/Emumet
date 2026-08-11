use super::delivery::deliver_activity_to_inbox;
use super::outbound_follow::find_existing_following;
use super::outbox::StoreOutboxActivityUseCase;
use super::remote_actor::{resolve_remote_actor_identifier, upsert_remote_account};
use super::{local_actor_url, ACTIVITYSTREAMS_CONTEXT};
use crate::transfer::activitypub::SendUndoFollowDto;
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use error_stack::{Report, ResultExt};
use kernel::activitypub::Activity;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::crypto::{DependOnKeyEncryptor, DependOnPasswordProvider};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::http_signing::DependOnHttpSigner;
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::repository::{
    DependOnFollowRepository, DependOnOutboxActivityRepository, DependOnRemoteAccountRepository,
    DependOnSigningKeyRepository, FollowRepository,
};
use kernel::prelude::entity::{
    Account, AuthAccountId, Follow, FollowTargetId, Nanoid, OutboxActivity, OutboxActivityId,
};
use kernel::KernelError;
use serde_json::Value;
use std::future::Future;

pub trait SendUndoFollowUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnFollowRepository
    + DependOnRemoteAccountRepository
    + DependOnSigningKeyRepository
    + DependOnHttpSigner
    + DependOnPasswordProvider
    + DependOnKeyEncryptor
    + DependOnPublicBaseUrl
    + DependOnOutboxActivityRepository
    + DependOnPermissionChecker
    + StoreOutboxActivityUseCase
{
    fn send_undo_follow(
        &self,
        auth_account_id: AuthAccountId,
        dto: SendUndoFollowDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send
    where
        Self: Sized,
    {
        async move {
            let account_nanoid = Nanoid::<Account>::new(dto.account_nanoid);
            let mut executor = self.database_connection().get_executor().await?;
            let account = self
                .account_query_processor()
                .find_by_nanoid(&mut executor, &account_nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        account_nanoid.as_ref()
                    ))
                })?;

            crate::permission::check_permission(
                self,
                &auth_account_id,
                &crate::permission::account_sign(account.id()),
            )
            .await?;

            let remote_actor = resolve_remote_actor_identifier(&dto.target).await?;
            let remote_account = upsert_remote_account(
                self.remote_account_repository(),
                &mut executor,
                remote_actor,
            )
            .await?;
            let source = FollowTargetId::from(account.id().clone());
            let destination = FollowTargetId::from(remote_account.id().clone());
            let follow = find_existing_following(
                self.follow_repository(),
                &mut executor,
                &source,
                &destination,
            )
            .await?
            .ok_or_else(|| {
                Report::new(KernelError::NotFound).attach_printable(format!(
                    "Follow relationship not found for {}",
                    remote_account.url().as_ref()
                ))
            })?;
            if follow.approved_at().is_none() {
                return Err(Report::new(KernelError::NotFound)
                    .attach_printable("Approved follow relationship not found"));
            }

            let local_actor_url =
                local_actor_url(self.public_base_url(), account.nanoid().as_ref());
            let activity = undo_follow_activity(
                self.public_base_url(),
                &follow,
                &local_actor_url,
                remote_account.url().as_ref(),
            )?;
            let inbox_url = remote_account.inbox_url().as_deref().ok_or_else(|| {
                Report::new(KernelError::Rejected)
                    .attach_printable("Remote actor does not expose an inbox URL")
            })?;

            deliver_activity_to_inbox(self, account.id(), inbox_url, &activity, "Undo").await?;
            self.follow_repository()
                .delete(&mut executor, follow.id())
                .await?;

            let outbox_entry = OutboxActivity {
                id: OutboxActivityId::default(),
                account_id: account.id().clone(),
                activity_id: activity.id.clone(),
                activity_type: "Undo".to_string(),
                object_json: serde_json::to_string(&activity).map_err(|error| {
                    Report::new(KernelError::Internal).attach_printable(format!(
                        "Failed to serialize Undo activity to JSON: {error}"
                    ))
                })?,
                created_at: time::OffsetDateTime::now_utc(),
            };
            self.store_outbox_activity(&outbox_entry)
                .await
                .change_context_lazy(|| KernelError::Internal)
                .attach_printable("Failed to store outbox activity")?;
            Ok(())
        }
    }
}

impl<T> SendUndoFollowUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnFollowRepository
        + DependOnRemoteAccountRepository
        + DependOnSigningKeyRepository
        + DependOnHttpSigner
        + DependOnPasswordProvider
        + DependOnKeyEncryptor
        + DependOnPublicBaseUrl
        + DependOnOutboxActivityRepository
        + DependOnPermissionChecker
        + StoreOutboxActivityUseCase
{
}

fn undo_follow_activity(
    public_base_url: &kernel::interfaces::config::PublicBaseUrl,
    follow: &Follow,
    local_actor_url: &str,
    remote_actor_url: &str,
) -> error_stack::Result<Activity, KernelError> {
    let original_follow = Activity {
        context: Some(Value::String(ACTIVITYSTREAMS_CONTEXT.to_string())),
        id: format!(
            "{}/activities/{}",
            public_base_url.as_str().trim_end_matches('/'),
            follow.id().as_ref()
        ),
        type_: "Follow".to_string(),
        actor: local_actor_url.to_string(),
        object: Some(Value::String(remote_actor_url.to_string())),
        target: None,
        to: Some(vec![remote_actor_url.to_string()]),
        cc: None,
    };
    let object = serde_json::to_value(original_follow).map_err(|error| {
        Report::new(KernelError::Internal)
            .attach_printable(format!("Failed to serialize original Follow: {error}"))
    })?;
    Ok(Activity {
        context: Some(Value::String(ACTIVITYSTREAMS_CONTEXT.to_string())),
        id: format!(
            "{}/activities/{}",
            public_base_url.as_str().trim_end_matches('/'),
            kernel::generate_id()
        ),
        type_: "Undo".to_string(),
        actor: local_actor_url.to_string(),
        object: Some(object),
        target: None,
        to: Some(vec![remote_actor_url.to_string()]),
        cc: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::interfaces::config::PublicBaseUrl;
    use kernel::prelude::entity::{AccountId, FollowId, RemoteAccountId};

    #[test]
    fn undo_wraps_the_original_follow_activity() {
        kernel::ensure_generator_initialized();
        let follow = Follow::new(
            FollowId::new(kernel::generate_id()),
            FollowTargetId::from(AccountId::default()),
            FollowTargetId::from(RemoteAccountId::new(kernel::generate_id())),
            None,
        )
        .unwrap();

        let undo = undo_follow_activity(
            &PublicBaseUrl::new("https://local.example".to_string()),
            &follow,
            "https://local.example/ap/accounts/alice",
            "https://remote.example/users/bob",
        )
        .unwrap();

        assert_eq!(undo.type_, "Undo");
        let original = undo.object.unwrap();
        assert_eq!(original["type"], "Follow");
        assert_eq!(original["actor"], "https://local.example/ap/accounts/alice");
        assert_eq!(original["object"], "https://remote.example/users/bob");
        assert!(original["id"]
            .as_str()
            .is_some_and(|id| id.ends_with(follow.id().as_ref().to_string().as_str())));
    }
}
