use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{
    AuthAccount, AuthAccountEvent, CommandEnvelope, EventEnvelope, EventId, EventVersion,
};
use crate::KernelError;
use std::future::Future;

pub trait AuthAccountEventStore: Sync + Send + 'static {
    type Executor: Executor;

    fn persist(
        &self,
        executor: &mut Self::Executor,
        command: &CommandEnvelope<AuthAccountEvent, AuthAccount>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn persist_and_transform(
        &self,
        executor: &mut Self::Executor,
        command: CommandEnvelope<AuthAccountEvent, AuthAccount>,
    ) -> impl Future<
        Output = error_stack::Result<EventEnvelope<AuthAccountEvent, AuthAccount>, KernelError>,
    > + Send;

    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &EventId<AuthAccountEvent, AuthAccount>,
        since: Option<&EventVersion<AuthAccount>>,
    ) -> impl Future<
        Output = error_stack::Result<
            Vec<EventEnvelope<AuthAccountEvent, AuthAccount>>,
            KernelError,
        >,
    > + Send;
}

pub trait DependOnAuthAccountEventStore: Sync + Send + DependOnDatabaseConnection {
    type AuthAccountEventStore: AuthAccountEventStore<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn auth_account_event_store(&self) -> &Self::AuthAccountEventStore;
}
