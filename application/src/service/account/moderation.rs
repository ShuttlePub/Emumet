use crate::permission::{check_permission, instance_moderate};
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::repository::{AggregateRepository, DependOnAccountRepository};
use kernel::prelude::entity::{Account, AuthAccountId, ModerationReason, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait SuspendAccountUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQueryProcessor
    + DependOnAccountRepository
    + DependOnTransactionManager
    + DependOnPermissionChecker
{
    fn suspend_account<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        account_id: String,
        reason: String,
        expires_at: Option<time::OffsetDateTime>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            ModerationReason::new(reason.as_str()).validate()?;
            let mut conn = self.database_connection().connection().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let projection = self
                .account_query_processor()
                .find_by_nanoid_unfiltered(&mut conn, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &instance_moderate()).await?;

            if let Some(exp) = expires_at {
                if exp <= time::OffsetDateTime::now_utc() {
                    return Err(Report::new(KernelError::Rejected)
                        .attach_printable("expires_at must be in the future"));
                }
            }

            let account_id = projection.id().clone();
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let (account, current_version) = deps
                            .account_repository()
                            .load(executor, &account_id)
                            .await?
                            .into_parts();

                        if !account.status().is_active() {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Account is not active"));
                        }
                        if account.deleted_at().is_some() {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Account is deactivated"));
                        }

                        deps.account_repository()
                            .save(
                                executor,
                                Account::suspend(account_id, reason, expires_at, current_version),
                            )
                            .await?;
                        Ok(())
                    })
                })
                .await?;

            Ok(())
        }
    }
}

impl<T> SuspendAccountUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountQueryProcessor
        + DependOnAccountRepository
        + DependOnTransactionManager
        + DependOnPermissionChecker
{
}

pub trait UnsuspendAccountUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQueryProcessor
    + DependOnAccountRepository
    + DependOnTransactionManager
    + DependOnPermissionChecker
{
    fn unsuspend_account<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            let mut conn = self.database_connection().connection().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let projection = self
                .account_query_processor()
                .find_by_nanoid_unfiltered(&mut conn, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &instance_moderate()).await?;

            let account_id = projection.id().clone();
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let (account, current_version) = deps
                            .account_repository()
                            .load(executor, &account_id)
                            .await?
                            .into_parts();

                        if !account.status().is_suspended() {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Account is not suspended"));
                        }

                        deps.account_repository()
                            .save(executor, Account::unsuspend(account_id, current_version))
                            .await?;
                        Ok(())
                    })
                })
                .await?;

            Ok(())
        }
    }
}

impl<T> UnsuspendAccountUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountQueryProcessor
        + DependOnAccountRepository
        + DependOnTransactionManager
        + DependOnPermissionChecker
{
}

pub trait BanAccountUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQueryProcessor
    + DependOnAccountRepository
    + DependOnTransactionManager
    + DependOnPermissionChecker
{
    fn ban_account<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        account_id: String,
        reason: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            ModerationReason::new(reason.as_str()).validate()?;
            let mut conn = self.database_connection().connection().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let projection = self
                .account_query_processor()
                .find_by_nanoid_unfiltered(&mut conn, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &instance_moderate()).await?;

            let account_id = projection.id().clone();
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let (account, current_version) = deps
                            .account_repository()
                            .load(executor, &account_id)
                            .await?
                            .into_parts();

                        if account.status().is_banned() {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Account is already banned"));
                        }
                        if account.deleted_at().is_some() {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Account is deactivated"));
                        }

                        deps.account_repository()
                            .save(executor, Account::ban(account_id, reason, current_version))
                            .await?;
                        Ok(())
                    })
                })
                .await?;

            Ok(())
        }
    }
}

impl<T> BanAccountUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountQueryProcessor
        + DependOnAccountRepository
        + DependOnTransactionManager
        + DependOnPermissionChecker
{
}
