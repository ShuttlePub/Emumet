use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::{StellarAccount, StellarAccountId};
use crate::KernelError;

pub trait StellarAccountQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &StellarAccountId,
    ) -> error_stack::Result<Option<StellarAccount>, KernelError>;
}

pub trait DependOnStellarAccountQuery: Sync + Send + DependOnDatabaseConnection {
    type StellarAccountQuery: StellarAccountQuery<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn stellar_account_query(&self) -> &Self::StellarAccountQuery;
}
