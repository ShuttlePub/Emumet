use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{Account, AccountId, AccountName, StellarAccountId};
use crate::KernelError;

pub trait AccountQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError>;

    async fn find_by_stellar_id(
        &self,
        transaction: &mut Self::Transaction,
        stellar_id: &StellarAccountId,
    ) -> error_stack::Result<Vec<Account>, KernelError>;

    async fn find_by_name(
        &self,
        transaction: &mut Self::Transaction,
        name: &AccountName,
    ) -> error_stack::Result<Option<Account>, KernelError>;
}

pub trait DependOnAccountQuery: Sync + Send + DependOnDatabaseConnection {
    type AccountQuery: AccountQuery<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn account_query(&self) -> &Self::AccountQuery;
}
