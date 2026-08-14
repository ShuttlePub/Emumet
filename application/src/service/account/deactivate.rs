use crate::permission::{account_deactivate, check_permission};
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::permission::{
    AccountRelation, DependOnPermissionChecker, DependOnPermissionWriter, PermissionWriter,
    RelationTarget,
};
use kernel::interfaces::repository::{AggregateRepository, DependOnAccountRepository};
use kernel::prelude::entity::{Account, AuthAccountId, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait DeactivateAccountUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQueryProcessor
    + DependOnAccountRepository
    + DependOnTransactionManager
    + DependOnPermissionChecker
    + DependOnPermissionWriter
{
    fn deactivate_account<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            let mut conn = self.database_connection().connection().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let projection = self
                .account_query_processor()
                .find_by_nanoid(&mut conn, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &account_deactivate(projection.id())).await?;

            let account_id = projection.id().clone();
            let transaction_account_id = account_id.clone();
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let current_version = deps
                            .account_repository()
                            .load(executor, &transaction_account_id)
                            .await?
                            .into_parts()
                            .1;
                        deps.account_repository()
                            .save(
                                executor,
                                Account::deactivate(transaction_account_id, current_version),
                            )
                            .await?;
                        Ok(())
                    })
                })
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
        + Clone
        + DependOnAccountQueryProcessor
        + DependOnAccountRepository
        + DependOnTransactionManager
        + DependOnPermissionChecker
        + DependOnPermissionWriter
{
}
