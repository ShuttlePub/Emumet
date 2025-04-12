use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{CommandEnvelope, EventEnvelope};
use crate::KernelError;
use serde::Serialize;
use std::future::Future;

pub trait EventModifier: 'static + Sync + Send {
    type Transaction: Transaction;

    fn persist<Event: Serialize + Sync, Entity: Sync>(
        &self,
        transaction: &mut Self::Transaction,
        command: &CommandEnvelope<Event, Entity>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn persist_and_transform<Event: Serialize + Sync + Send, Entity: Sync + Send>(
        &self,
        transaction: &mut Self::Transaction,
        command: CommandEnvelope<Event, Entity>,
    ) -> impl Future<Output = error_stack::Result<EventEnvelope<Event, Entity>, KernelError>> + Send;
}

pub trait DependOnEventModifier: Sync + Send + DependOnDatabaseConnection {
    type EventModifier: EventModifier<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn event_modifier(&self) -> &Self::EventModifier;
}
