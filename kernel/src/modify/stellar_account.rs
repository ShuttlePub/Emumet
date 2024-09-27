use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::{StellarAccount, StellarAccountId};
use crate::KernelError;
use std::future::Future;

pub trait StellarAccountModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    fn create(
        &self,
        transaction: &mut Self::Transaction,
        stellar_account: &StellarAccount,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        transaction: &mut Self::Transaction,
        stellar_account: &StellarAccount,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &StellarAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnStellarAccountModifier: Sync + Send + DependOnDatabaseConnection {
    type StellarAccountModifier: StellarAccountModifier<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn stellar_account_modifier(&self) -> &Self::StellarAccountModifier;
}
