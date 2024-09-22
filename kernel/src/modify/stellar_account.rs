use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::{StellarAccount, StellarAccountId};
use crate::KernelError;

pub trait StellarAccountModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn create(
        &self,
        transaction: &mut Self::Transaction,
        stellar_account: &StellarAccount,
    ) -> error_stack::Result<(), KernelError>;

    async fn update(
        &self,
        transaction: &mut Self::Transaction,
        stellar_account: &StellarAccount,
    ) -> error_stack::Result<(), KernelError>;

    async fn delete(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &StellarAccountId,
    ) -> error_stack::Result<(), KernelError>;
}

pub trait DependOnStellarAccountModifier: Sync + Send + DependOnDatabaseConnection {
    type StellarAccountModifier: StellarAccountModifier<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn stellar_account_modifier(&self) -> &Self::StellarAccountModifier;
}
