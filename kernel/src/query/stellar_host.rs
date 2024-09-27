use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{StellarHost, StellarHostId, StellarHostUrl};
use crate::KernelError;
use std::future::Future;

pub trait StellarHostQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &StellarHostId,
    ) -> impl Future<Output = error_stack::Result<Option<StellarHost>, KernelError>> + Send;

    fn find_by_url(
        &self,
        transaction: &mut Self::Transaction,
        domain: &StellarHostUrl,
    ) -> impl Future<Output = error_stack::Result<Option<StellarHost>, KernelError>> + Send;
}

pub trait DependOnStellarHostQuery: Sync + Send + DependOnDatabaseConnection {
    type StellarHostQuery: StellarHostQuery<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn stellar_host_query(&self) -> &Self::StellarHostQuery;
}
