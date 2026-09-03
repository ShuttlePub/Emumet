use super::profile_transfer_request_event_store::PostgresProfileTransferRequestEventStore;
use crate::database::{PostgresConnection, PostgresDatabase};
use error_stack::Report;
use kernel::interfaces::event_store::ProfileTransferRequestEventStore;
use kernel::interfaces::repository::{
    AggregateRepository, DependOnProfileTransferRequestRepository, Rehydrated,
};
use kernel::prelude::entity::{
    CommandEnvelope, EventEnvelope, EventId, ProfileTransferRequest, ProfileTransferRequestEvent,
    ProfileTransferRequestId,
};
use kernel::KernelError;

pub struct PostgresProfileTransferRequestRepository;

impl AggregateRepository<ProfileTransferRequest> for PostgresProfileTransferRequestRepository {
    type Connection = PostgresConnection;
    type Id = ProfileTransferRequestId;

    async fn load(
        &self,
        executor: &mut Self::Connection,
        id: &Self::Id,
    ) -> error_stack::Result<Rehydrated<ProfileTransferRequest>, KernelError> {
        let events = PostgresProfileTransferRequestEventStore
            .find_by_id(executor, &EventId::from(id.clone()), None)
            .await?;
        Rehydrated::<ProfileTransferRequest>::from_events(events)?.ok_or_else(|| {
            Report::new(KernelError::NotFound).attach_printable(format!(
                "No events found for profile transfer request: {}",
                id.as_ref()
            ))
        })
    }

    async fn save(
        &self,
        executor: &mut Self::Connection,
        command: CommandEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest>,
    ) -> error_stack::Result<
        EventEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest>,
        KernelError,
    > {
        PostgresProfileTransferRequestEventStore
            .persist_and_transform(executor, command)
            .await
    }
}

impl DependOnProfileTransferRequestRepository for PostgresDatabase {
    type ProfileTransferRequestRepository = PostgresProfileTransferRequestRepository;

    fn profile_transfer_request_repository(&self) -> &Self::ProfileTransferRequestRepository {
        &PostgresProfileTransferRequestRepository
    }
}
