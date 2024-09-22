use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::CommandEnvelope;
use crate::KernelError;
use serde::Serialize;

pub trait EventModifier: 'static + Sync + Send {
    type Transaction: Transaction;

    async fn handle<Event: Serialize, Entity>(
        &self,
        transaction: &mut Self::Transaction,
        event: &CommandEnvelope<Event, Entity>,
    ) -> error_stack::Result<(), KernelError>;
}

pub trait DependOnEventModifier: Sync + Send + DependOnDatabaseConnection {
    type EventModifier: EventModifier<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn event_modifier(&self) -> &Self::EventModifier;
}
