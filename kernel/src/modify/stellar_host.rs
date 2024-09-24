use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::StellarHost;
use crate::KernelError;

pub trait StellarHostModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn create(
        &self,
        transaction: &mut Self::Transaction,
        stellar_host: &StellarHost,
    ) -> error_stack::Result<(), KernelError>;

    async fn update(
        &self,
        transaction: &mut Self::Transaction,
        stellar_host: &StellarHost,
    ) -> error_stack::Result<(), KernelError>;
}

pub trait DependOnStellarHostModifier: Sync + Send + DependOnDatabaseConnection {
    type StellarHostModifier: StellarHostModifier<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn stellar_host_modifier(&self) -> &Self::StellarHostModifier;
}
