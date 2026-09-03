use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    CommandEnvelope, EventEnvelope, EventId, EventVersion, ProfileTransferRequest,
    ProfileTransferRequestEvent,
};
use crate::KernelError;
use std::future::Future;

pub trait ProfileTransferRequestEventStore: Sync + Send + 'static {
    type Connection: Connection;

    fn persist(
        &self,
        executor: &mut Self::Connection,
        command: &CommandEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn persist_and_transform(
        &self,
        executor: &mut Self::Connection,
        command: CommandEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest>,
    ) -> impl Future<
        Output = error_stack::Result<
            EventEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest>,
            KernelError,
        >,
    > + Send;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &EventId<ProfileTransferRequestEvent, ProfileTransferRequest>,
        since: Option<&EventVersion<ProfileTransferRequest>>,
    ) -> impl Future<
        Output = error_stack::Result<
            Vec<EventEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest>>,
            KernelError,
        >,
    > + Send;
}

pub trait DependOnProfileTransferRequestEventStore:
    Sync + Send + DependOnDatabaseConnection
{
    type ProfileTransferRequestEventStore: ProfileTransferRequestEventStore<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_transfer_request_event_store(&self) -> &Self::ProfileTransferRequestEventStore;
}
