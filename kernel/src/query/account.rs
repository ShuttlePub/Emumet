use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{Account, AccountId, AccountName, StellarAccountId};
use crate::KernelError;
use std::future::Future;

pub trait AccountQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_stellar_id(
        &self,
        transaction: &mut Self::Transaction,
        stellar_id: &StellarAccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<Account>, KernelError>> + Send;

    fn find_by_name(
        &self,
        transaction: &mut Self::Transaction,
        name: &AccountName,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;
}

pub trait DependOnAccountQuery: Sync + Send + DependOnDatabaseConnection {
    type AccountQuery: AccountQuery<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn account_query(&self) -> &Self::AccountQuery;
}
