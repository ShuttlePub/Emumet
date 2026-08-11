use crate::transfer::activitypub::FollowRelationDto;
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, Executor};
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::repository::{
    DependOnFollowRepository, DependOnRemoteAccountRepository, FollowRepository,
    RemoteAccountRepository,
};
use kernel::prelude::entity::{Account, AuthAccountId, FollowTargetId, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait GetFollowRelationsUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnFollowRepository
    + DependOnRemoteAccountRepository
    + DependOnPermissionChecker
{
    fn get_followers(
        &self,
        auth_account_id: AuthAccountId,
        account_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<Vec<FollowRelationDto>, KernelError>> + Send
    where
        Self: Sized,
    {
        async move {
            self.get_follow_relations(auth_account_id, account_nanoid, false)
                .await
        }
    }

    fn get_following(
        &self,
        auth_account_id: AuthAccountId,
        account_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<Vec<FollowRelationDto>, KernelError>> + Send
    where
        Self: Sized,
    {
        async move {
            self.get_follow_relations(auth_account_id, account_nanoid, true)
                .await
        }
    }

    fn get_follow_relations(
        &self,
        auth_account_id: AuthAccountId,
        account_nanoid: String,
        following: bool,
    ) -> impl Future<Output = error_stack::Result<Vec<FollowRelationDto>, KernelError>> + Send
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
            list_approved_relations(
                self.account_query_processor(),
                self.follow_repository(),
                self.remote_account_repository(),
                &mut executor,
                &FollowTargetId::from(account.id().clone()),
                following,
            )
            .await
        }
    }
}

impl<T> GetFollowRelationsUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnFollowRepository
        + DependOnRemoteAccountRepository
        + DependOnPermissionChecker
{
}

pub async fn list_approved_relations<Q, F, R, E>(
    accounts: &Q,
    follows: &F,
    remote_accounts: &R,
    executor: &mut E,
    account: &FollowTargetId,
    following: bool,
) -> error_stack::Result<Vec<FollowRelationDto>, KernelError>
where
    Q: AccountQueryProcessor<Executor = E>,
    F: FollowRepository<Executor = E>,
    R: RemoteAccountRepository<Executor = E>,
    E: Executor,
{
    let relations = if following {
        follows.find_followings(executor, account).await?
    } else {
        follows.find_followers(executor, account).await?
    };
    let mut result = Vec::with_capacity(relations.len());
    for relation in relations
        .into_iter()
        .filter(|relation| relation.approved_at().is_some())
    {
        let target = if following {
            relation.destination()
        } else {
            relation.source()
        };
        let (target_type, target) = match target {
            FollowTargetId::Local(account_id) => {
                let target = accounts
                    .find_by_id(executor, account_id)
                    .await?
                    .ok_or_else(|| {
                        Report::new(KernelError::Internal).attach_printable(format!(
                            "Follow relation local account not found: {}",
                            account_id.as_ref()
                        ))
                    })?;
                ("local", target.nanoid().as_ref().to_string())
            }
            FollowTargetId::Remote(account_id) => {
                let target = remote_accounts
                    .find_by_id(executor, account_id)
                    .await?
                    .ok_or_else(|| {
                        Report::new(KernelError::Internal).attach_printable(format!(
                            "Follow relation remote account not found: {}",
                            account_id.as_ref()
                        ))
                    })?;
                ("remote", target.url().as_ref().to_string())
            }
        };
        result.push(FollowRelationDto {
            id: relation.id().as_ref().to_string(),
            target_type: target_type.to_string(),
            target,
        });
    }
    Ok(result)
}
