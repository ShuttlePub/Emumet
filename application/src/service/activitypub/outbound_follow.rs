use super::delivery::deliver_activity_to_inbox;
use super::outbox::StoreOutboxActivityUseCase;
use super::remote_actor::{resolve_remote_actor_identifier, upsert_remote_account};
use super::{local_actor_url, ACTIVITYSTREAMS_CONTEXT};
use crate::transfer::activitypub::{SendFollowDto, SendFollowResultDto};
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
    Account, AccountId, Follow, FollowId, FollowTargetId, Nanoid, OutboxActivity, OutboxActivityId,
};
use kernel::KernelError;
use serde_json::Value;
use std::future::Future;

pub trait SendFollowUseCase:
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
    fn send_follow(
        &self,
        auth_account_id: kernel::prelude::entity::AuthAccountId,
        dto: SendFollowDto,
    ) -> impl Future<Output = error_stack::Result<SendFollowResultDto, KernelError>> + Send
    where
        Self: Sized,
    {
        async move {
            let account_nanoid = Nanoid::<Account>::new(dto.account_nanoid.clone());
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

            if let Some(_existing) = find_existing_following(
                self.follow_repository(),
                &mut executor,
                &source,
                &destination,
            )
            .await?
            {
                return Err(Report::new(KernelError::Rejected).attach_printable(format!(
                    "Already following {}",
                    remote_account.url().as_ref()
                )));
            }

            let follow = Follow::new(
                FollowId::new(kernel::generate_id()),
                source,
                destination,
                None,
            )?;
            self.follow_repository()
                .create(&mut executor, &follow)
                .await?;

            let local_actor_url =
                local_actor_url(self.public_base_url(), account.nanoid().as_ref());
            let follow_activity = follow_activity(
                self.public_base_url(),
                &follow,
                &local_actor_url,
                remote_account.url().as_ref(),
            )?;

            if let Err(error) = self
                .deliver_follow(account.id(), remote_account.inbox_url(), &follow_activity)
                .await
            {
                self.follow_repository()
                    .delete(&mut executor, follow.id())
                    .await?;
                return Err(error
                    .change_context(KernelError::Rejected)
                    .attach_printable("Follow delivery failed, rolled back follow record"));
            }

            let outbox_entry = OutboxActivity {
                id: OutboxActivityId::default(),
                account_id: account.id().clone(),
                activity_id: follow_activity.id.clone(),
                activity_type: "Follow".to_string(),
                object_json: serde_json::to_string(&follow_activity).map_err(|e| {
                    Report::new(KernelError::Internal).attach_printable(format!(
                        "Failed to serialize Follow activity to JSON: {e}"
                    ))
                })?,
                created_at: time::OffsetDateTime::now_utc(),
            };
            self.store_outbox_activity(&outbox_entry)
                .await
                .change_context_lazy(|| KernelError::Internal)
                .attach_printable("Failed to store outbox activity")?;

            Ok(SendFollowResultDto {
                follow_id: follow.id().as_ref().to_string(),
                remote_actor_url: remote_account.url().as_ref().to_string(),
                activity_id: follow_activity.id.clone(),
                approved: false,
            })
        }
    }

    fn deliver_follow(
        &self,
        account_id: &AccountId,
        inbox_url: &Option<String>,
        activity: &Activity,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send
    where
        Self: Sized,
    {
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
                activity,
                "Follow",
            )
            .await
        }
    }
}

impl<T> SendFollowUseCase for T where
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

pub(super) async fn find_existing_following<R, E>(
    repository: &R,
    executor: &mut E,
    source: &FollowTargetId,
    destination: &FollowTargetId,
) -> error_stack::Result<Option<Follow>, KernelError>
where
    R: FollowRepository<Executor = E>,
    E: kernel::interfaces::database::Executor,
{
    let followings = repository.find_followings(executor, source).await?;
    Ok(followings
        .into_iter()
        .find(|follow| follow.source() == source && follow.destination() == destination))
}

fn follow_activity(
    public_base_url: &kernel::interfaces::config::PublicBaseUrl,
    follow: &Follow,
    local_actor_url: &str,
    remote_actor_url: &str,
) -> error_stack::Result<Activity, KernelError> {
    Ok(Activity {
        context: Some(Value::String(ACTIVITYSTREAMS_CONTEXT.to_string())),
        id: format!(
            "{}/activities/{}",
            public_base_url.as_str().trim_end_matches('/'),
            follow.id().as_ref()
        ),
        type_: "Follow".to_string(),
        actor: local_actor_url.to_string(),
        object: Some(serde_json::Value::String(remote_actor_url.to_string())),
        target: None,
        to: Some(vec![remote_actor_url.to_string()]),
        cc: None,
    })
}
