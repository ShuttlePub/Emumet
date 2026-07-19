use super::rehydrate::rehydrate_account;
use crate::permission::{account_edit, check_permission};
use crate::transfer::account::UpdateAccountDto;
use adapter::processor::account::{
    AccountCommandProcessor, AccountQueryProcessor, DependOnAccountCommandProcessor,
    DependOnAccountQueryProcessor, UpdateAccountParam,
};
use error_stack::Report;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::event_store::DependOnAccountEventStore;
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::prelude::entity::{Account, AccountIsBot, AuthAccountId, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait UpdateAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountCommandProcessor
    + DependOnAccountQueryProcessor
    + DependOnAccountEventStore
    + DependOnPermissionChecker
{
    fn update_account(
        &self,
        auth_account_id: &AuthAccountId,
        dto: UpdateAccountDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

            let nanoid = Nanoid::<Account>::new(dto.account_nanoid);
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

            check_permission(self, auth_account_id, &account_edit(projection.id())).await?;

            let account_id = projection.id().clone();
            let (account, current_version) =
                rehydrate_account(self, &mut transaction, &account_id).await?;

            let is_bot = match dto.is_bot {
                kernel::prelude::entity::FieldAction::Unchanged => *account.is_bot().as_ref(),
                kernel::prelude::entity::FieldAction::Clear => false,
                kernel::prelude::entity::FieldAction::Set(value) => value,
            };

            if !account.status().is_active() {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("Cannot modify a suspended or banned account"));
            }
            if account.deleted_at().is_some() {
                return Err(
                    Report::new(KernelError::Rejected).attach_printable("Account is deactivated")
                );
            }

            self.account_command_processor()
                .update(
                    &mut transaction,
                    UpdateAccountParam {
                        account_id,
                        is_bot: AccountIsBot::new(is_bot),
                        current_version,
                    },
                )
                .await?;

            Ok(())
        }
    }
}

impl<T> UpdateAccountUseCase for T where
    T: 'static
        + DependOnAccountCommandProcessor
        + DependOnAccountQueryProcessor
        + DependOnAccountEventStore
        + DependOnPermissionChecker
{
}
