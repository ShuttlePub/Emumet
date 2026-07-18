use super::rehydrate::rehydrate_account;
use crate::permission::{check_permission, instance_moderate};
use adapter::processor::account::{
    AccountCommandProcessor, AccountQueryProcessor, DependOnAccountCommandProcessor,
    DependOnAccountQueryProcessor,
};
use error_stack::Report;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::event_store::DependOnAccountEventStore;
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::prelude::entity::{Account, AuthAccountId, ModerationReason, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait SuspendAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountCommandProcessor
    + DependOnAccountQueryProcessor
    + DependOnAccountEventStore
    + DependOnPermissionChecker
{
    fn suspend_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
        reason: String,
        expires_at: Option<time::OffsetDateTime>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            ModerationReason::new(reason.as_str()).validate()?;
            let mut transaction = self.database_connection().get_executor().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let projection = self
                .account_query_processor()
                .find_by_nanoid_unfiltered(&mut transaction, &nanoid)
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
            let (account, current_version) =
                rehydrate_account(self, &mut transaction, &account_id).await?;

            if !account.status().is_active() {
                return Err(
                    Report::new(KernelError::Rejected).attach_printable("Account is not active")
                );
            }
            if account.deleted_at().is_some() {
                return Err(
                    Report::new(KernelError::Rejected).attach_printable("Account is deactivated")
                );
            }

            self.account_command_processor()
                .suspend(
                    &mut transaction,
                    account_id,
                    reason,
                    expires_at,
                    current_version,
                )
                .await?;

            Ok(())
        }
    }
}

impl<T> SuspendAccountUseCase for T where
    T: 'static
        + DependOnAccountCommandProcessor
        + DependOnAccountQueryProcessor
        + DependOnAccountEventStore
        + DependOnPermissionChecker
{
}

pub trait UnsuspendAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountCommandProcessor
    + DependOnAccountQueryProcessor
    + DependOnAccountEventStore
    + DependOnPermissionChecker
{
    fn unsuspend_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let projection = self
                .account_query_processor()
                .find_by_nanoid_unfiltered(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &instance_moderate()).await?;

            let account_id = projection.id().clone();
            let (account, current_version) =
                rehydrate_account(self, &mut transaction, &account_id).await?;

            if !account.status().is_suspended() {
                return Err(
                    Report::new(KernelError::Rejected).attach_printable("Account is not suspended")
                );
            }

            self.account_command_processor()
                .unsuspend(&mut transaction, account_id, current_version)
                .await?;

            Ok(())
        }
    }
}

impl<T> UnsuspendAccountUseCase for T where
    T: 'static
        + DependOnAccountCommandProcessor
        + DependOnAccountQueryProcessor
        + DependOnAccountEventStore
        + DependOnPermissionChecker
{
}

pub trait BanAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountCommandProcessor
    + DependOnAccountQueryProcessor
    + DependOnAccountEventStore
    + DependOnPermissionChecker
{
    fn ban_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
        reason: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            ModerationReason::new(reason.as_str()).validate()?;
            let mut transaction = self.database_connection().get_executor().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let projection = self
                .account_query_processor()
                .find_by_nanoid_unfiltered(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &instance_moderate()).await?;

            let account_id = projection.id().clone();
            let (account, current_version) =
                rehydrate_account(self, &mut transaction, &account_id).await?;

            if account.status().is_banned() {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("Account is already banned"));
            }
            if account.deleted_at().is_some() {
                return Err(
                    Report::new(KernelError::Rejected).attach_printable("Account is deactivated")
                );
            }

            self.account_command_processor()
                .ban(&mut transaction, account_id, reason, current_version)
                .await?;

            Ok(())
        }
    }
}

impl<T> BanAccountUseCase for T where
    T: 'static
        + DependOnAccountCommandProcessor
        + DependOnAccountQueryProcessor
        + DependOnAccountEventStore
        + DependOnPermissionChecker
{
}
