use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{Account, AccountId, CommandEnvelope, EventEnvelope, EventVersion};
use crate::event::EventApplier;
use crate::KernelError;
use std::future::Future;

/// Boundary model returned by `AggregateRepository::load` (ADR 0006 decision 9):
/// the rehydrated aggregate and the version it was loaded at, kept separate.
#[derive(Debug, Clone)]
pub struct Rehydrated<A> {
    aggregate: A,
    version: EventVersion<A>,
}

impl<A> Rehydrated<A> {
    pub fn new(aggregate: A, version: EventVersion<A>) -> Self {
        Self { aggregate, version }
    }

    pub fn aggregate(&self) -> &A {
        &self.aggregate
    }

    pub fn version(&self) -> &EventVersion<A> {
        &self.version
    }

    pub fn into_parts(self) -> (A, EventVersion<A>) {
        (self.aggregate, self.version)
    }
}

impl<A> Rehydrated<A>
where
    A: EventApplier,
{
    /// Fold a persisted event stream into the aggregate.
    /// Returns `Ok(None)` when the stream is empty.
    pub fn from_events(
        events: Vec<EventEnvelope<A::Event, A>>,
    ) -> error_stack::Result<Option<Rehydrated<A>>, KernelError> {
        let Some(version) = events
            .last()
            .map(|event| EventVersion::new(*event.version.as_ref()))
        else {
            return Ok(None);
        };
        let mut aggregate: Option<A> = None;
        for event in events {
            A::apply(&mut aggregate, event)?;
        }
        let aggregate = aggregate.ok_or_else(|| {
            error_stack::Report::new(KernelError::Internal)
                .attach_printable("event stream folded into an empty aggregate")
        })?;
        Ok(Some(Rehydrated::new(aggregate, version)))
    }
}

/// Repository port for event-sourced aggregates (ADR 0006 decision 3).
///
/// An aggregate change is expressed as a `CommandEnvelope`, which carries the
/// `ExpectedVersion` used for optimistic concurrency. Implementations append
/// events only; they participate in an outer transaction and never begin or
/// commit their own (ADR 0006 decision 2).
pub trait AggregateRepository<A>: Sync + Send + 'static
where
    A: EventApplier,
{
    type Connection: Connection;
    type Id;

    fn load(
        &self,
        executor: &mut Self::Connection,
        id: &Self::Id,
    ) -> impl Future<Output = error_stack::Result<Rehydrated<A>, KernelError>> + Send;

    fn save(
        &self,
        executor: &mut Self::Connection,
        command: CommandEnvelope<A::Event, A>,
    ) -> impl Future<Output = error_stack::Result<EventEnvelope<A::Event, A>, KernelError>> + Send;
}

pub trait DependOnAccountRepository: Sync + Send + DependOnDatabaseConnection {
    type AccountRepository: AggregateRepository<
        Account,
        Id = AccountId,
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn account_repository(&self) -> &Self::AccountRepository;
}
