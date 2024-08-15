use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{Account, AccountEvent, AccountId, CommandEnvelope};
use crate::KernelError;

pub trait AccountModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn create(
        &self,
        transaction: &mut Self::Transaction,
        account: &Account,
    ) -> error_stack::Result<(), KernelError>;

    async fn update(
        &self,
        transaction: &mut Self::Transaction,
        account: &Account,
    ) -> error_stack::Result<(), KernelError>;

    async fn delete(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError>;
}

pub trait DependOnAccountModifier: Sync + Send + DependOnDatabaseConnection {
    type AccountModifier: AccountModifier<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn account_modifier(&self) -> &Self::AccountModifier;
}

pub trait AccountEventModifier: 'static + Sync + Send {
    type Transaction: Transaction;

    async fn handle(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
        event: &CommandEnvelope<AccountEvent, Account>,
    ) -> error_stack::Result<(), KernelError>;
}

pub trait DependOnAccountEventModifier: Sync + Send + DependOnDatabaseConnection {
    type AccountEventModifier: AccountEventModifier<Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction>;

    fn account_event_modifier(&self) -> &Self::AccountEventModifier;
}
