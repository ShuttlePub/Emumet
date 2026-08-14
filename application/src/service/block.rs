use crate::service::activitypub::outbound_block::{block_activity, undo_block_activity};
use crate::service::activitypub::remote_actor::{
    resolve_remote_actor_identifier, upsert_remote_account,
};
use crate::service::activitypub::{
    local_actor_url, DeliverOutboxActivityUseCase, StoreOutboxActivityUseCase,
};
use crate::transfer::block_mute::{BlockAccountDto, RelationDto};
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use error_stack::Report;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::crypto::{DependOnKeyEncryptor, DependOnPasswordProvider};
use kernel::interfaces::database::{
    Connection, DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::http_signing::DependOnHttpSigner;
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::repository::{
    BlockRepository, DependOnBlockRepository, DependOnFollowRepository,
    DependOnOutboxActivityRepository, DependOnRemoteAccountRepository,
    DependOnSigningKeyRepository, FollowRepository, OutboxActivityRepository,
    RemoteAccountRepository,
};
use kernel::prelude::entity::{
    Account, AuthAccountId, Block, BlockId, BlockTargetId, FollowTargetId, Nanoid, OutboxActivity,
    RemoteAccount,
};
use kernel::KernelError;
use std::future::Future;

pub trait BlockAccountUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQueryProcessor
    + DependOnBlockRepository
    + DependOnFollowRepository
    + DependOnRemoteAccountRepository
    + DependOnSigningKeyRepository
    + DependOnHttpSigner
    + DependOnPasswordProvider
    + DependOnKeyEncryptor
    + DependOnPublicBaseUrl
    + DependOnOutboxActivityRepository
    + DependOnPermissionChecker
    + DependOnTransactionManager
    + StoreOutboxActivityUseCase
{
    fn block_account(
        &self,
        auth_account_id: AuthAccountId,
        dto: BlockAccountDto,
    ) -> impl Future<Output = error_stack::Result<RelationDto, KernelError>> + Send
    where
        Self: Sized,
    {
        async move {
            let account_nanoid = Nanoid::<Account>::new(dto.account_nanoid.clone());
            let mut executor = self.database_connection().connection().await?;
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

            let source = BlockTargetId::from(account.id().clone());
            let resolved = resolve_block_target(
                self.account_query_processor(),
                self.remote_account_repository(),
                &mut executor,
                &dto.target,
            )
            .await?;
            let (destination, target) = block_target_parts(&resolved);

            if source == destination {
                return Err(
                    Report::new(KernelError::Rejected).attach_printable("Cannot block yourself")
                );
            }

            let local_actor_url =
                local_actor_url(self.public_base_url(), account.nanoid().as_ref());
            let block_id = BlockId::new(kernel::generate_id());
            let delivered_activity = match &resolved {
                BlockTarget::Local(_) => None,
                BlockTarget::Remote(remote_account) => {
                    let activity = block_activity(
                        self.public_base_url(),
                        &block_id,
                        &local_actor_url,
                        remote_account.url().as_ref(),
                    );
                    Some((activity, remote_account.inbox_url().clone()))
                }
            };

            let deps = self.clone();
            let account_id = account.id().clone();
            let source_for_tx = source.clone();
            let destination_for_tx = destination.clone();
            let block = Block::new(block_id, source, destination)?;
            let target_type = match block.destination() {
                BlockTargetId::Local(_) => "local",
                BlockTargetId::Remote(_) => "remote",
            };
            let block_id_for_result = block.id().as_ref().to_string();
            let account_id_for_delivery = account_id.clone();
            let (outbox_id, delivered_activity) = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let existing = deps
                            .block_repository()
                            .find_blocks(executor, &source_for_tx)
                            .await?;
                        if existing
                            .iter()
                            .any(|block| block.destination() == &destination_for_tx)
                        {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Already blocked"));
                        }

                        deps.block_repository().create(executor, &block).await?;

                        let follow_source = block_target_to_follow_target(&source_for_tx);
                        let follow_destination = block_target_to_follow_target(&destination_for_tx);
                        remove_follows_between(
                            deps.follow_repository(),
                            executor,
                            &follow_source,
                            &follow_destination,
                        )
                        .await?;

                        let outbox_id = if let Some((activity, _)) = &delivered_activity {
                            let outbox_entry = OutboxActivity {
                                id: 0,
                                account_id: account_id.clone(),
                                activity_id: activity.id.clone(),
                                activity_type: "Block".to_string(),
                                object_json: serde_json::to_string(&activity).map_err(|error| {
                                    Report::new(KernelError::Internal).attach_printable(format!(
                                        "Failed to serialize Block activity to JSON: {error}"
                                    ))
                                })?,
                                created_at: time::OffsetDateTime::now_utc(),
                                delivered_at: None,
                                attempted_at: None,
                                error: None,
                            };
                            Some(
                                deps.outbox_activity_repository()
                                    .create(executor, &outbox_entry)
                                    .await?,
                            )
                        } else {
                            None
                        };

                        Ok((outbox_id, delivered_activity))
                    })
                })
                .await?;

            if let (Some(outbox_id), Some((activity, inbox_url))) = (outbox_id, delivered_activity)
            {
                if let Some(inbox_url) = inbox_url.as_deref() {
                    if let Err(error) = self
                        .deliver_outbox_activity(
                            &outbox_id,
                            &account_id_for_delivery,
                            inbox_url,
                            &activity,
                            "Block",
                        )
                        .await
                    {
                        tracing::warn!(?error, inbox_url, "Failed to deliver ActivityPub Block");
                    }
                }
            }

            Ok(RelationDto {
                id: block_id_for_result,
                target_type: target_type.to_string(),
                target,
            })
        }
    }
}

impl<T> BlockAccountUseCase for T where
    T: 'static
        + Clone
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnBlockRepository
        + DependOnFollowRepository
        + DependOnRemoteAccountRepository
        + DependOnSigningKeyRepository
        + DependOnHttpSigner
        + DependOnPasswordProvider
        + DependOnKeyEncryptor
        + DependOnPublicBaseUrl
        + DependOnOutboxActivityRepository
        + DependOnPermissionChecker
        + DependOnTransactionManager
        + StoreOutboxActivityUseCase
{
}

pub trait UnblockAccountUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQueryProcessor
    + DependOnBlockRepository
    + DependOnRemoteAccountRepository
    + DependOnSigningKeyRepository
    + DependOnHttpSigner
    + DependOnPasswordProvider
    + DependOnKeyEncryptor
    + DependOnPublicBaseUrl
    + DependOnOutboxActivityRepository
    + DependOnPermissionChecker
    + DependOnTransactionManager
    + StoreOutboxActivityUseCase
    + DeliverOutboxActivityUseCase
{
    fn unblock_account(
        &self,
        auth_account_id: AuthAccountId,
        dto: BlockAccountDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send
    where
        Self: Sized,
    {
        async move {
            let account_nanoid = Nanoid::<Account>::new(dto.account_nanoid.clone());
            let mut executor = self.database_connection().connection().await?;
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

            let source = BlockTargetId::from(account.id().clone());
            let resolved = resolve_block_target(
                self.account_query_processor(),
                self.remote_account_repository(),
                &mut executor,
                &dto.target,
            )
            .await?;
            let (destination, _) = block_target_parts(&resolved);

            let deps = self.clone();
            let account_id = account.id().clone();
            let account_id_for_delivery = account_id.clone();
            let local_actor_url =
                local_actor_url(self.public_base_url(), account.nanoid().as_ref());
            let (outbox_id, delivered_activity) = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let blocks = deps
                            .block_repository()
                            .find_blocks(executor, &source)
                            .await?;
                        let block = blocks
                            .into_iter()
                            .find(|block| block.destination() == &destination)
                            .ok_or_else(|| {
                                Report::new(KernelError::NotFound)
                                    .attach_printable("Block relationship not found")
                            })?;

                        let delivered_activity = match &resolved {
                            BlockTarget::Local(_) => None,
                            BlockTarget::Remote(remote_account) => {
                                let original_block = block_activity(
                                    deps.public_base_url(),
                                    block.id(),
                                    &local_actor_url,
                                    remote_account.url().as_ref(),
                                );
                                let undo = undo_block_activity(
                                    deps.public_base_url(),
                                    original_block,
                                    remote_account.url().as_ref(),
                                )?;
                                Some((undo, remote_account.inbox_url().clone()))
                            }
                        };

                        deps.block_repository().delete(executor, block.id()).await?;

                        let outbox_id = if let Some((activity, _)) = &delivered_activity {
                            let outbox_entry = OutboxActivity {
                                id: 0,
                                account_id: account_id.clone(),
                                activity_id: activity.id.clone(),
                                activity_type: "Undo".to_string(),
                                object_json: serde_json::to_string(&activity).map_err(|error| {
                                    Report::new(KernelError::Internal).attach_printable(format!(
                                        "Failed to serialize Undo activity to JSON: {error}"
                                    ))
                                })?,
                                created_at: time::OffsetDateTime::now_utc(),
                                delivered_at: None,
                                attempted_at: None,
                                error: None,
                            };
                            Some(
                                deps.outbox_activity_repository()
                                    .create(executor, &outbox_entry)
                                    .await?,
                            )
                        } else {
                            None
                        };

                        Ok((outbox_id, delivered_activity))
                    })
                })
                .await?;

            if let (Some(outbox_id), Some((activity, inbox_url))) = (outbox_id, delivered_activity)
            {
                if let Some(inbox_url) = inbox_url.as_deref() {
                    if let Err(error) = self
                        .deliver_outbox_activity(
                            &outbox_id,
                            &account_id_for_delivery,
                            inbox_url,
                            &activity,
                            "Undo",
                        )
                        .await
                    {
                        tracing::warn!(
                            ?error,
                            inbox_url,
                            "Failed to deliver ActivityPub Undo(Block)"
                        );
                    }
                }
            }
            Ok(())
        }
    }
}

impl<T> UnblockAccountUseCase for T where
    T: 'static
        + Clone
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnBlockRepository
        + DependOnRemoteAccountRepository
        + DependOnSigningKeyRepository
        + DependOnHttpSigner
        + DependOnPasswordProvider
        + DependOnKeyEncryptor
        + DependOnPublicBaseUrl
        + DependOnOutboxActivityRepository
        + DependOnPermissionChecker
        + DependOnTransactionManager
        + StoreOutboxActivityUseCase
        + DeliverOutboxActivityUseCase
{
}

pub trait GetBlocksUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnBlockRepository
    + DependOnRemoteAccountRepository
    + DependOnPermissionChecker
{
    fn get_blocks(
        &self,
        auth_account_id: AuthAccountId,
        account_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<Vec<RelationDto>, KernelError>> + Send
    where
        Self: Sized,
    {
        async move {
            let account_nanoid = Nanoid::<Account>::new(account_nanoid);
            let mut executor = self.database_connection().connection().await?;
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

            let source = BlockTargetId::from(account.id().clone());
            let blocks = self
                .block_repository()
                .find_blocks(&mut executor, &source)
                .await?;

            let mut relations = Vec::with_capacity(blocks.len());
            for block in blocks {
                let relation = match block.destination() {
                    BlockTargetId::Local(account_id) => {
                        let target_account = self
                            .account_query_processor()
                            .find_by_id(&mut executor, account_id)
                            .await?
                            .ok_or_else(|| {
                                Report::new(KernelError::Internal).attach_printable(format!(
                                    "Blocked local account not found: {}",
                                    account_id.as_ref()
                                ))
                            })?;
                        RelationDto {
                            id: block.id().as_ref().to_string(),
                            target_type: "local".to_string(),
                            target: target_account.nanoid().as_ref().to_string(),
                        }
                    }
                    BlockTargetId::Remote(remote_account_id) => {
                        let remote_account = self
                            .remote_account_repository()
                            .find_by_id(&mut executor, remote_account_id)
                            .await?
                            .ok_or_else(|| {
                                Report::new(KernelError::Internal).attach_printable(format!(
                                    "Blocked remote account not found: {}",
                                    remote_account_id.as_ref()
                                ))
                            })?;
                        RelationDto {
                            id: block.id().as_ref().to_string(),
                            target_type: "remote".to_string(),
                            target: remote_account.url().as_ref().to_string(),
                        }
                    }
                };
                relations.push(relation);
            }
            Ok(relations)
        }
    }
}

impl<T> GetBlocksUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnBlockRepository
        + DependOnRemoteAccountRepository
        + DependOnPermissionChecker
{
}

pub async fn remove_follows_between<R, E>(
    repository: &R,
    executor: &mut E,
    a: &FollowTargetId,
    b: &FollowTargetId,
) -> error_stack::Result<(), KernelError>
where
    R: FollowRepository<Connection = E>,
    E: Connection,
{
    let followings_of_a = repository.find_followings(executor, a).await?;
    for follow in followings_of_a
        .into_iter()
        .filter(|follow| follow.destination() == b)
    {
        repository.delete(executor, follow.id()).await?;
    }
    let followings_of_b = repository.find_followings(executor, b).await?;
    for follow in followings_of_b
        .into_iter()
        .filter(|follow| follow.destination() == a)
    {
        repository.delete(executor, follow.id()).await?;
    }
    Ok(())
}

pub(crate) fn block_target_to_follow_target(target: &BlockTargetId) -> FollowTargetId {
    match target {
        BlockTargetId::Local(account_id) => FollowTargetId::Local(account_id.clone()),
        BlockTargetId::Remote(remote_account_id) => {
            FollowTargetId::Remote(remote_account_id.clone())
        }
    }
}

pub(crate) enum BlockTarget {
    Local(Account),
    Remote(RemoteAccount),
}

fn block_target_parts(target: &BlockTarget) -> (BlockTargetId, String) {
    match target {
        BlockTarget::Local(account) => (
            BlockTargetId::from(account.id().clone()),
            account.nanoid().as_ref().to_string(),
        ),
        BlockTarget::Remote(remote_account) => (
            BlockTargetId::from(remote_account.id().clone()),
            remote_account.url().as_ref().to_string(),
        ),
    }
}

async fn resolve_block_target<Q, R>(
    query_processor: &Q,
    remote_account_repository: &R,
    executor: &mut Q::Connection,
    target: &str,
) -> error_stack::Result<BlockTarget, KernelError>
where
    Q: AccountQueryProcessor,
    R: RemoteAccountRepository<Connection = Q::Connection>,
{
    let target_nanoid = Nanoid::<Account>::new(target.to_string());
    if let Some(account) = query_processor
        .find_by_nanoid(executor, &target_nanoid)
        .await?
    {
        return Ok(BlockTarget::Local(account));
    }
    let actor = resolve_remote_actor_identifier(target).await?;
    let remote_account = upsert_remote_account(remote_account_repository, executor, actor).await?;
    Ok(BlockTarget::Remote(remote_account))
}
