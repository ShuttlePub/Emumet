use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::{CommandEnvelope, StellarAccount, StellarAccountEvent, StellarAccountId};
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

pub trait StellarAccountEventModifier: 'static + Sync + Send {
    type Transaction: Transaction;

    async fn handle(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &StellarAccountId,
        event: &CommandEnvelope<StellarAccountEvent, StellarAccount>,
    ) -> error_stack::Result<(), KernelError>;
}

pub trait DependOnStellarAccountEventModifier: Sync + Send + DependOnDatabaseConnection {
    type StellarAccountEventModifier: StellarAccountEventModifier<Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction>;

    fn stellar_account_event_modifier(&self) -> &Self::StellarAccountEventModifier;
}
