use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{
    CommandEnvelope, EventEnvelope, EventId, EventVersion, Metadata, MetadataEvent,
};
use crate::KernelError;
use std::future::Future;

pub trait MetadataEventStore: Sync + Send + 'static {
    type Executor: Executor;

    fn persist(
        &self,
        executor: &mut Self::Executor,
        command: &CommandEnvelope<MetadataEvent, Metadata>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn persist_and_transform(
        &self,
        executor: &mut Self::Executor,
        command: CommandEnvelope<MetadataEvent, Metadata>,
    ) -> impl Future<Output = error_stack::Result<EventEnvelope<MetadataEvent, Metadata>, KernelError>>
           + Send;

    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &EventId<MetadataEvent, Metadata>,
        since: Option<&EventVersion<Metadata>>,
    ) -> impl Future<
        Output = error_stack::Result<Vec<EventEnvelope<MetadataEvent, Metadata>>, KernelError>,
    > + Send;
}

pub trait DependOnMetadataEventStore: Sync + Send + DependOnDatabaseConnection {
    type MetadataEventStore: MetadataEventStore<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn metadata_event_store(&self) -> &Self::MetadataEventStore;
}
