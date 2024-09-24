use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{StellarHost, StellarHostId, StellarHostUrl};
use crate::KernelError;

pub trait StellarHostQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &StellarHostId,
    ) -> error_stack::Result<Option<StellarHost>, KernelError>;

    async fn find_by_url(
        &self,
        transaction: &mut Self::Transaction,
        domain: &StellarHostUrl,
    ) -> error_stack::Result<Option<StellarHost>, KernelError>;
}

pub trait DependOnStellarHostQuery: Sync + Send + DependOnDatabaseConnection {
    type StellarHostQuery: StellarHostQuery<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn stellar_host_query(&self) -> &Self::StellarHostQuery;
}
