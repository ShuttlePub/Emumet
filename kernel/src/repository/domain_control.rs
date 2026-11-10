use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{AccountId, ControlDomain, DomainControl, DomainControlMode, ProfileId};
use crate::KernelError;
use std::future::Future;

/// Result of a revision-checked delete.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DomainControlDeleteOutcome {
    Deleted,
    /// No live control exists for the Profile x domain.
    NotFound,
    /// A live control exists but its revision differs from the expected one.
    RevisionMismatch,
}

/// Storage of Profile x domain controls.
///
/// Every write is a single atomic statement, so concurrent writers racing on the same
/// `(profile, domain)` are serialized by the database and exactly one of them wins.
/// Deleted controls keep a tombstone (the revision counter) so revisions never repeat.
pub trait DomainControlRepository: Sync + Send + 'static {
    type Connection: Connection;

    /// Read the Profile's **current** owning account and hold a shared row lock on the Profile
    /// until the surrounding transaction ends. A concurrent Profile transfer (which updates the
    /// same row) therefore waits for the transaction, and a transfer that committed first is
    /// observed here, so an owner check made with this lock cannot be invalidated before the
    /// transaction's reads / writes finish.
    fn lock_profile_owner(
        &self,
        executor: &mut Self::Connection,
        profile_nanoid: &str,
    ) -> impl Future<Output = error_stack::Result<Option<(ProfileId, AccountId)>, KernelError>> + Send;

    /// The live control for the Profile x domain, if any.
    fn find(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
        domain: &ControlDomain,
    ) -> impl Future<Output = error_stack::Result<Option<DomainControl>, KernelError>> + Send;

    /// All live controls of the Profile ordered by domain.
    fn find_by_profile(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<Vec<DomainControl>, KernelError>> + Send;

    /// Create a control (revision 1, or the previous revision + 1 after a delete).
    /// `None` when a live control already exists.
    fn create(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
        domain: &ControlDomain,
        mode: DomainControlMode,
    ) -> impl Future<Output = error_stack::Result<Option<DomainControl>, KernelError>> + Send;

    /// Replace the mode of the live control iff its revision equals `expected_revision`
    /// (revision + 1). `None` when there is no live control or the revision differs.
    fn replace(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
        domain: &ControlDomain,
        mode: DomainControlMode,
        expected_revision: u64,
    ) -> impl Future<Output = error_stack::Result<Option<DomainControl>, KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
        domain: &ControlDomain,
        expected_revision: u64,
    ) -> impl Future<Output = error_stack::Result<DomainControlDeleteOutcome, KernelError>> + Send;
}

pub trait DependOnDomainControlRepository: Sync + Send + DependOnDatabaseConnection {
    type DomainControlRepository: DomainControlRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn domain_control_repository(&self) -> &Self::DomainControlRepository;
}
