use super::rehydrate::rehydrate_account;
use crate::permission::{account_deactivate, check_permission};
use adapter::processor::account::{
    AccountCommandProcessor, AccountQueryProcessor, DependOnAccountCommandProcessor,
    DependOnAccountQueryProcessor,
};
use error_stack::Report;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::event_store::DependOnAccountEventStore;
use kernel::interfaces::permission::{
    AccountRelation, DependOnPermissionChecker, DependOnPermissionWriter, PermissionWriter,
    RelationTarget,
};
use kernel::prelude::entity::{Account, AuthAccountId, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait DeactivateAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountCommandProcessor
    + DependOnAccountQueryProcessor
    + DependOnAccountEventStore
    + DependOnPermissionChecker
    + DependOnPermissionWriter
{
    fn deactivate_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let projection = self
                .account_query_processor()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &account_deactivate(projection.id())).await?;

            let account_id = projection.id().clone();
            let (_account, current_version) =
                rehydrate_account(self, &mut transaction, &account_id).await?;
            self.account_command_processor()
                .deactivate(&mut transaction, account_id.clone(), current_version)
                .await?;

            for relation in [
                AccountRelation::Owner,
                AccountRelation::Editor,
                AccountRelation::Signer,
            ] {
                self.permission_writer()
                    .delete_relation(
                        &RelationTarget::Account {
                            account_id: account_id.clone(),
                            relation,
                        },
                        auth_account_id,
                    )
                    .await?;
            }

            Ok(())
        }
    }
}

impl<T> DeactivateAccountUseCase for T where
    T: 'static
        + DependOnAccountCommandProcessor
        + DependOnAccountQueryProcessor
        + DependOnAccountEventStore
        + DependOnPermissionChecker
        + DependOnPermissionWriter
{
}
