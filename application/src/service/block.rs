use crate::service::activitypub::remote_actor::{
    resolve_remote_actor_identifier, upsert_remote_account,
};
use crate::transfer::block_mute::{BlockAccountDto, RelationDto};
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, Executor};
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::repository::{
    BlockRepository, DependOnBlockRepository, DependOnFollowRepository,
    DependOnRemoteAccountRepository, FollowRepository, RemoteAccountRepository,
};
use kernel::prelude::entity::{
    Account, AuthAccountId, Block, BlockId, BlockTargetId, FollowTargetId, Nanoid,
};
use kernel::KernelError;
use std::future::Future;

pub trait BlockAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnBlockRepository
    + DependOnFollowRepository
    + DependOnRemoteAccountRepository
    + DependOnPermissionChecker
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

            let source = BlockTargetId::from(account.id().clone());
            let (destination, target) = resolve_block_target(
                self.account_query_processor(),
                self.remote_account_repository(),
                &mut executor,
                &dto.target,
            )
            .await?;

            if source == destination {
                return Err(
                    Report::new(KernelError::Rejected).attach_printable("Cannot block yourself")
                );
            }

            let existing = self
                .block_repository()
                .find_blocks(&mut executor, &source)
                .await?;
            if existing
                .iter()
                .any(|block| block.destination() == &destination)
            {
                return Err(Report::new(KernelError::Rejected).attach_printable("Already blocked"));
            }

            let block = Block::new(
                BlockId::new(kernel::generate_id()),
                source.clone(),
                destination.clone(),
            )?;
            self.block_repository()
                .create(&mut executor, &block)
                .await?;

            let follow_source = block_target_to_follow_target(&source);
            let follow_destination = block_target_to_follow_target(&destination);
            remove_follows_between(
                self.follow_repository(),
                &mut executor,
                &follow_source,
                &follow_destination,
            )
            .await?;

            let target_type = match &destination {
                BlockTargetId::Local(_) => "local",
                BlockTargetId::Remote(_) => "remote",
            };
            Ok(RelationDto {
                id: block.id().as_ref().to_string(),
                target_type: target_type.to_string(),
                target,
            })
        }
    }
}

impl<T> BlockAccountUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnBlockRepository
        + DependOnFollowRepository
        + DependOnRemoteAccountRepository
        + DependOnPermissionChecker
{
}

pub trait UnblockAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnBlockRepository
    + DependOnRemoteAccountRepository
    + DependOnPermissionChecker
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

            let source = BlockTargetId::from(account.id().clone());
            let (destination, _) = resolve_block_target(
                self.account_query_processor(),
                self.remote_account_repository(),
                &mut executor,
                &dto.target,
            )
            .await?;

            let blocks = self
                .block_repository()
                .find_blocks(&mut executor, &source)
                .await?;
            let block = blocks
                .into_iter()
                .find(|block| block.destination() == &destination)
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable("Block relationship not found")
                })?;
            self.block_repository()
                .delete(&mut executor, block.id())
                .await?;
            Ok(())
        }
    }
}

impl<T> UnblockAccountUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnBlockRepository
        + DependOnRemoteAccountRepository
        + DependOnPermissionChecker
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
    R: FollowRepository<Executor = E>,
    E: Executor,
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

fn block_target_to_follow_target(target: &BlockTargetId) -> FollowTargetId {
    match target {
        BlockTargetId::Local(account_id) => FollowTargetId::Local(account_id.clone()),
        BlockTargetId::Remote(remote_account_id) => {
            FollowTargetId::Remote(remote_account_id.clone())
        }
    }
}

async fn resolve_block_target<Q, R>(
    query_processor: &Q,
    remote_account_repository: &R,
    executor: &mut Q::Executor,
    target: &str,
) -> error_stack::Result<(BlockTargetId, String), KernelError>
where
    Q: AccountQueryProcessor,
    R: RemoteAccountRepository<Executor = Q::Executor>,
{
    let target_nanoid = Nanoid::<Account>::new(target.to_string());
    if let Some(account) = query_processor
        .find_by_nanoid(executor, &target_nanoid)
        .await?
    {
        return Ok((
            BlockTargetId::from(account.id().clone()),
            account.nanoid().as_ref().to_string(),
        ));
    }
    let actor = resolve_remote_actor_identifier(target).await?;
    let remote_account = upsert_remote_account(remote_account_repository, executor, actor).await?;
    Ok((
        BlockTargetId::from(remote_account.id().clone()),
        remote_account.url().as_ref().to_string(),
    ))
}
