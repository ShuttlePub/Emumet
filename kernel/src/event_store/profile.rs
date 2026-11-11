use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    CommandEnvelope, EventEnvelope, EventId, EventVersion, Profile, ProfileEvent, ProfileUsername,
};
use crate::KernelError;
use std::future::Future;

pub trait ProfileEventStore: Sync + Send + 'static {
    type Connection: Connection;

    fn persist(
        &self,
        executor: &mut Self::Connection,
        command: &CommandEnvelope<ProfileEvent, Profile>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn persist_and_transform(
        &self,
        executor: &mut Self::Connection,
        command: CommandEnvelope<ProfileEvent, Profile>,
    ) -> impl Future<Output = error_stack::Result<EventEnvelope<ProfileEvent, Profile>, KernelError>>
           + Send;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &EventId<ProfileEvent, Profile>,
        since: Option<&EventVersion<Profile>>,
    ) -> impl Future<
        Output = error_stack::Result<Vec<EventEnvelope<ProfileEvent, Profile>>, KernelError>,
    > + Send;

    /// Whether any Profile event ever carried `username` (`Created` with a username or
    /// `UsernameSet`), including events of Profiles that were deleted since.
    ///
    /// This is the same source of truth as the `profile_events_username_unique` index that makes
    /// creation and assignment fail with `Concurrency` (409): a username for which this returns
    /// `true` can never be assigned again. Read-only; used for availability checks only. The
    /// answer is advisory, because another request may take the username right after the read.
    fn username_exists(
        &self,
        executor: &mut Self::Connection,
        username: &ProfileUsername,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;
}

pub trait DependOnProfileEventStore: Sync + Send + DependOnDatabaseConnection {
    type ProfileEventStore: ProfileEventStore<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_event_store(&self) -> &Self::ProfileEventStore;
}
