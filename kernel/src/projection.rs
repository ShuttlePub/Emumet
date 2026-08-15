use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    Account, AccountEvent, EventEnvelope, Metadata, MetadataEvent, MetadataId, Profile,
    ProfileEvent, ProfileId,
};
use crate::KernelError;
use std::future::Future;

/// One row of a transactional event log: the global `seq` plus the envelope.
///
/// `seq` (BIGSERIAL) is the commit-independent global order assigned at INSERT
/// time; `EventEnvelope::version` is the per-aggregate order. The projector
/// must fold per-aggregate events by `version` and must never rely on `seq`
/// order across aggregates (ADR 0006 decision 4).
#[derive(Debug, Clone)]
pub struct SeqEvent<Event, Entity> {
    pub seq: i64,
    pub envelope: EventEnvelope<Event, Entity>,
}

/// Transactional log tailing read for account events (ADR 0006 decision 4).
pub trait AccountEventLog: Sync + Send + 'static {
    type Connection: Connection;

    /// Read the committed account events whose `seq` is strictly greater than
    /// `from_seq_exclusive`, in `seq` order, at most `limit` rows.
    fn find_by_seq_window(
        &self,
        executor: &mut Self::Connection,
        from_seq_exclusive: i64,
        limit: i64,
    ) -> impl Future<Output = error_stack::Result<Vec<SeqEvent<AccountEvent, Account>>, KernelError>>
           + Send;
}

pub trait DependOnAccountEventLog: Sync + Send + DependOnDatabaseConnection {
    type AccountEventLog: AccountEventLog<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn account_event_log(&self) -> &Self::AccountEventLog;
}

/// Checkpoint persistence for transactional log tailing projectors.
pub trait ProjectionCheckpointStore: Sync + Send + 'static {
    type Connection: Connection;

    fn get(
        &self,
        executor: &mut Self::Connection,
        projector_name: &str,
    ) -> impl Future<Output = error_stack::Result<Option<i64>, KernelError>> + Send;

    /// Advance the checkpoint. Implementations must be monotonic: a stored
    /// checkpoint never regresses, so a late `set` with an older `seq` is a
    /// no-op.
    fn set(
        &self,
        executor: &mut Self::Connection,
        projector_name: &str,
        seq: i64,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnProjectionCheckpointStore: Sync + Send + DependOnDatabaseConnection {
    type ProjectionCheckpointStore: ProjectionCheckpointStore<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn projection_checkpoint_store(&self) -> &Self::ProjectionCheckpointStore;
}

/// Version-gated projection writer for accounts (ADR 0006 decision 3).
///
/// A single upsert replaces the read-model `create`/`update` pair for the
/// projector: insert the row when absent, and when present apply the write
/// only if the incoming aggregate version is strictly newer than the stored
/// one. Re-applying an already-projected event is therefore a no-op.
pub trait AccountProjectionWriter: Sync + Send + 'static {
    type Connection: Connection;

    fn upsert(
        &self,
        executor: &mut Self::Connection,
        account: &Account,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnAccountProjectionWriter: Sync + Send + DependOnDatabaseConnection {
    type AccountProjectionWriter: AccountProjectionWriter<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn account_projection_writer(&self) -> &Self::AccountProjectionWriter;
}

/// Transactional log tailing read for profile events (ADR 0006 decision 4).
pub trait ProfileEventLog: Sync + Send + 'static {
    type Connection: Connection;

    fn find_by_seq_window(
        &self,
        executor: &mut Self::Connection,
        from_seq_exclusive: i64,
        limit: i64,
    ) -> impl Future<Output = error_stack::Result<Vec<SeqEvent<ProfileEvent, Profile>>, KernelError>>
           + Send;
}

pub trait DependOnProfileEventLog: Sync + Send + DependOnDatabaseConnection {
    type ProfileEventLog: ProfileEventLog<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_event_log(&self) -> &Self::ProfileEventLog;
}

/// Version-gated projection writer for profiles.
pub trait ProfileProjectionWriter: Sync + Send + 'static {
    type Connection: Connection;

    fn upsert(
        &self,
        executor: &mut Self::Connection,
        profile: &Profile,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnProfileProjectionWriter: Sync + Send + DependOnDatabaseConnection {
    type ProfileProjectionWriter: ProfileProjectionWriter<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_projection_writer(&self) -> &Self::ProfileProjectionWriter;
}

/// Transactional log tailing read for metadata events (ADR 0006 decision 4).
pub trait MetadataEventLog: Sync + Send + 'static {
    type Connection: Connection;

    fn find_by_seq_window(
        &self,
        executor: &mut Self::Connection,
        from_seq_exclusive: i64,
        limit: i64,
    ) -> impl Future<Output = error_stack::Result<Vec<SeqEvent<MetadataEvent, Metadata>>, KernelError>>
           + Send;
}

pub trait DependOnMetadataEventLog: Sync + Send + DependOnDatabaseConnection {
    type MetadataEventLog: MetadataEventLog<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn metadata_event_log(&self) -> &Self::MetadataEventLog;
}

/// Version-gated projection writer for metadata.
pub trait MetadataProjectionWriter: Sync + Send + 'static {
    type Connection: Connection;

    fn upsert(
        &self,
        executor: &mut Self::Connection,
        metadata: &Metadata,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Connection,
        metadata_id: &MetadataId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnMetadataProjectionWriter: Sync + Send + DependOnDatabaseConnection {
    type MetadataProjectionWriter: MetadataProjectionWriter<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn metadata_projection_writer(&self) -> &Self::MetadataProjectionWriter;
}
