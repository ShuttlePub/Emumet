use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{CommandEnvelope, EventEnvelope};
use crate::KernelError;
use serde::Serialize;
use std::future::Future;

pub trait EventModifier: 'static + Sync + Send {
    type Executor: Executor;

    fn persist<Event: Serialize + Sync, Entity: Sync>(
        &self,
        executor: &mut Self::Executor,
        command: &CommandEnvelope<Event, Entity>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn persist_and_transform<Event: Serialize + Sync + Send, Entity: Sync + Send>(
        &self,
        executor: &mut Self::Executor,
        command: CommandEnvelope<Event, Entity>,
    ) -> impl Future<Output = error_stack::Result<EventEnvelope<Event, Entity>, KernelError>> + Send;
}

pub trait DependOnEventModifier: Sync + Send + DependOnDatabaseConnection {
    type EventModifier: EventModifier<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn event_modifier(&self) -> &Self::EventModifier;
}
