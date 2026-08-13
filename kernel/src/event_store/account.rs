use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{Account, AccountEvent, CommandEnvelope, EventEnvelope, EventId, EventVersion};
use crate::KernelError;
use std::future::Future;

pub trait AccountEventStore: Sync + Send + 'static {
    type Connection: Connection;

    fn persist(
        &self,
        executor: &mut Self::Connection,
        command: &CommandEnvelope<AccountEvent, Account>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn persist_and_transform(
        &self,
        executor: &mut Self::Connection,
        command: CommandEnvelope<AccountEvent, Account>,
    ) -> impl Future<Output = error_stack::Result<EventEnvelope<AccountEvent, Account>, KernelError>>
           + Send;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &EventId<AccountEvent, Account>,
        since: Option<&EventVersion<Account>>,
    ) -> impl Future<
        Output = error_stack::Result<Vec<EventEnvelope<AccountEvent, Account>>, KernelError>,
    > + Send;
}

pub trait DependOnAccountEventStore: Sync + Send + DependOnDatabaseConnection {
    type AccountEventStore: AccountEventStore<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn account_event_store(&self) -> &Self::AccountEventStore;
}
