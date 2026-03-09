use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{Account, AccountEvent, CommandEnvelope, EventEnvelope, EventId, EventVersion};
use crate::KernelError;
use std::future::Future;

pub trait AccountEventStore: Sync + Send + 'static {
    type Transaction: Transaction;

    fn persist(
        &self,
        transaction: &mut Self::Transaction,
        command: &CommandEnvelope<AccountEvent, Account>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn persist_and_transform(
        &self,
        transaction: &mut Self::Transaction,
        command: CommandEnvelope<AccountEvent, Account>,
    ) -> impl Future<Output = error_stack::Result<EventEnvelope<AccountEvent, Account>, KernelError>>
           + Send;

    fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &EventId<AccountEvent, Account>,
        since: Option<&EventVersion<Account>>,
    ) -> impl Future<
        Output = error_stack::Result<Vec<EventEnvelope<AccountEvent, Account>>, KernelError>,
    > + Send;
}

pub trait DependOnAccountEventStore: Sync + Send + DependOnDatabaseConnection {
    type AccountEventStore: AccountEventStore<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn account_event_store(&self) -> &Self::AccountEventStore;
}
