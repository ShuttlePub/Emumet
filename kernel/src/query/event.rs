use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{EventEnvelope, EventId, EventVersion};
use crate::KernelError;
use serde::Deserialize;

pub trait EventQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn find_by_id<Event: for<'de> Deserialize<'de>, Entity>(
        &self,
        transaction: &mut Self::Transaction,
        id: &EventId<Event, Entity>,
        since: Option<&EventVersion<Entity>>,
    ) -> error_stack::Result<Vec<EventEnvelope<Event, Entity>>, KernelError>;
}

pub trait DependOnEventQuery: Sync + Send + DependOnDatabaseConnection {
    type EventQuery: EventQuery<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn event_query(&self) -> &Self::EventQuery;
}
