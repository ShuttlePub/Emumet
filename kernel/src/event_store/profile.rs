use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{CommandEnvelope, EventEnvelope, EventId, EventVersion, Profile, ProfileEvent};
use crate::KernelError;
use std::future::Future;

pub trait ProfileEventStore: Sync + Send + 'static {
    type Executor: Executor;

    fn persist(
        &self,
        executor: &mut Self::Executor,
        command: &CommandEnvelope<ProfileEvent, Profile>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn persist_and_transform(
        &self,
        executor: &mut Self::Executor,
        command: CommandEnvelope<ProfileEvent, Profile>,
    ) -> impl Future<Output = error_stack::Result<EventEnvelope<ProfileEvent, Profile>, KernelError>>
           + Send;

    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &EventId<ProfileEvent, Profile>,
        since: Option<&EventVersion<Profile>>,
    ) -> impl Future<
        Output = error_stack::Result<Vec<EventEnvelope<ProfileEvent, Profile>>, KernelError>,
    > + Send;
}

pub trait DependOnProfileEventStore: Sync + Send + DependOnDatabaseConnection {
    type ProfileEventStore: ProfileEventStore<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn profile_event_store(&self) -> &Self::ProfileEventStore;
}
