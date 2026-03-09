use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{Account, AccountId, AccountName, AuthAccountId, Nanoid};
use crate::KernelError;
use std::future::Future;

pub trait AccountReadModel: Sync + Send + 'static {
    type Transaction: Transaction;

    // Query operations (projection reads)
    fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_auth_id(
        &self,
        transaction: &mut Self::Transaction,
        auth_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<Account>, KernelError>> + Send;

    fn find_by_name(
        &self,
        transaction: &mut Self::Transaction,
        name: &AccountName,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_nanoid(
        &self,
        transaction: &mut Self::Transaction,
        nanoid: &Nanoid<Account>,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    // Projection update operations (called by EventApplier pipeline)
    fn create(
        &self,
        transaction: &mut Self::Transaction,
        account: &Account,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        transaction: &mut Self::Transaction,
        account: &Account,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn link_auth_account(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
        auth_account_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnAccountReadModel: Sync + Send + DependOnDatabaseConnection {
    type AccountReadModel: AccountReadModel<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn account_read_model(&self) -> &Self::AccountReadModel;
}
