use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{EventEnvelope, EventId, EventVersion};
use crate::KernelError;
use serde::Deserialize;
use std::future::Future;

pub trait EventQuery: Sync + Send + 'static {
    type Executor: Executor;

    fn find_by_id<Event: for<'de> Deserialize<'de> + Sync, Entity: Sync>(
        &self,
        executor: &mut Self::Executor,
        id: &EventId<Event, Entity>,
        since: Option<&EventVersion<Entity>>,
    ) -> impl Future<Output = error_stack::Result<Vec<EventEnvelope<Event, Entity>>, KernelError>> + Send;
}

pub trait DependOnEventQuery: Sync + Send + DependOnDatabaseConnection {
    type EventQuery: EventQuery<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn event_query(&self) -> &Self::EventQuery;
}
