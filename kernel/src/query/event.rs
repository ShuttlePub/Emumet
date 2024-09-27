use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{EventEnvelope, EventId, EventVersion};
use crate::KernelError;
use serde::Deserialize;
use std::future::Future;

pub trait EventQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    fn find_by_id<Event: for<'de> Deserialize<'de> + Sync, Entity: Sync>(
        &self,
        transaction: &mut Self::Transaction,
        id: &EventId<Event, Entity>,
        since: Option<&EventVersion<Entity>>,
    ) -> impl Future<Output = error_stack::Result<Vec<EventEnvelope<Event, Entity>>, KernelError>> + Send;
}

pub trait DependOnEventQuery: Sync + Send + DependOnDatabaseConnection {
    type EventQuery: EventQuery<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn event_query(&self) -> &Self::EventQuery;
}
