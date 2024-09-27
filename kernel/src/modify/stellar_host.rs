use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::StellarHost;
use crate::KernelError;
use std::future::Future;

pub trait StellarHostModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    fn create(
        &self,
        transaction: &mut Self::Transaction,
        stellar_host: &StellarHost,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        transaction: &mut Self::Transaction,
        stellar_host: &StellarHost,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnStellarHostModifier: Sync + Send + DependOnDatabaseConnection {
    type StellarHostModifier: StellarHostModifier<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn stellar_host_modifier(&self) -> &Self::StellarHostModifier;
}
