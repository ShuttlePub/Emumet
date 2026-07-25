use crate::service::activitypub::remote_actor::{
    resolve_remote_actor_identifier, upsert_remote_account,
};
use crate::transfer::block_mute::{MuteAccountDto, RelationDto};
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use error_stack::Report;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::repository::{
    DependOnMuteRepository, DependOnRemoteAccountRepository, MuteRepository,
    RemoteAccountRepository,
};
use kernel::prelude::entity::{Account, AuthAccountId, Mute, MuteId, MuteTargetId, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait MuteAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnMuteRepository
    + DependOnRemoteAccountRepository
    + DependOnPermissionChecker
{
    fn mute_account(
        &self,
        auth_account_id: AuthAccountId,
        dto: MuteAccountDto,
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

            let source = MuteTargetId::from(account.id().clone());
            let (destination, target) = resolve_mute_target(
                self.account_query_processor(),
                self.remote_account_repository(),
                &mut executor,
                &dto.target,
            )
            .await?;

            if source == destination {
                return Err(
                    Report::new(KernelError::Rejected).attach_printable("Cannot mute yourself")
                );
            }

            let existing = self
                .mute_repository()
                .find_mutes(&mut executor, &source)
                .await?;
            if existing
                .iter()
                .any(|mute| mute.destination() == &destination)
            {
                return Err(Report::new(KernelError::Rejected).attach_printable("Already muted"));
            }

            let mute = Mute::new(
                MuteId::new(kernel::generate_id()),
                source.clone(),
                destination.clone(),
            )?;
            self.mute_repository().create(&mut executor, &mute).await?;

            let target_type = match &destination {
                MuteTargetId::Local(_) => "local",
                MuteTargetId::Remote(_) => "remote",
            };
            Ok(RelationDto {
                id: mute.id().as_ref().to_string(),
                target_type: target_type.to_string(),
                target,
            })
        }
    }
}

impl<T> MuteAccountUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnMuteRepository
        + DependOnRemoteAccountRepository
        + DependOnPermissionChecker
{
}

pub trait UnmuteAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnMuteRepository
    + DependOnRemoteAccountRepository
    + DependOnPermissionChecker
{
    fn unmute_account(
        &self,
        auth_account_id: AuthAccountId,
        dto: MuteAccountDto,
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

            let source = MuteTargetId::from(account.id().clone());
            let (destination, _) = resolve_mute_target(
                self.account_query_processor(),
                self.remote_account_repository(),
                &mut executor,
                &dto.target,
            )
            .await?;

            let mutes = self
                .mute_repository()
                .find_mutes(&mut executor, &source)
                .await?;
            let mute = mutes
                .into_iter()
                .find(|mute| mute.destination() == &destination)
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable("Mute relationship not found")
                })?;
            self.mute_repository()
                .delete(&mut executor, mute.id())
                .await?;
            Ok(())
        }
    }
}

impl<T> UnmuteAccountUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnMuteRepository
        + DependOnRemoteAccountRepository
        + DependOnPermissionChecker
{
}

pub trait GetMutesUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnMuteRepository
    + DependOnRemoteAccountRepository
    + DependOnPermissionChecker
{
    fn get_mutes(
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

            let source = MuteTargetId::from(account.id().clone());
            let mutes = self
                .mute_repository()
                .find_mutes(&mut executor, &source)
                .await?;

            let mut relations = Vec::with_capacity(mutes.len());
            for mute in mutes {
                let relation = match mute.destination() {
                    MuteTargetId::Local(account_id) => {
                        let target_account = self
                            .account_query_processor()
                            .find_by_id(&mut executor, account_id)
                            .await?
                            .ok_or_else(|| {
                                Report::new(KernelError::Internal).attach_printable(format!(
                                    "Muted local account not found: {}",
                                    account_id.as_ref()
                                ))
                            })?;
                        RelationDto {
                            id: mute.id().as_ref().to_string(),
                            target_type: "local".to_string(),
                            target: target_account.nanoid().as_ref().to_string(),
                        }
                    }
                    MuteTargetId::Remote(remote_account_id) => {
                        let remote_account = self
                            .remote_account_repository()
                            .find_by_id(&mut executor, remote_account_id)
                            .await?
                            .ok_or_else(|| {
                                Report::new(KernelError::Internal).attach_printable(format!(
                                    "Muted remote account not found: {}",
                                    remote_account_id.as_ref()
                                ))
                            })?;
                        RelationDto {
                            id: mute.id().as_ref().to_string(),
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

impl<T> GetMutesUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnMuteRepository
        + DependOnRemoteAccountRepository
        + DependOnPermissionChecker
{
}

async fn resolve_mute_target<Q, R>(
    query_processor: &Q,
    remote_account_repository: &R,
    executor: &mut Q::Executor,
    target: &str,
) -> error_stack::Result<(MuteTargetId, String), KernelError>
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
            MuteTargetId::from(account.id().clone()),
            account.nanoid().as_ref().to_string(),
        ));
    }
    let actor = resolve_remote_actor_identifier(target).await?;
    let remote_account = upsert_remote_account(remote_account_repository, executor, actor).await?;
    Ok((
        MuteTargetId::from(remote_account.id().clone()),
        remote_account.url().as_ref().to_string(),
    ))
}
