use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    AccountReport, AccountReportEvent, CommandEnvelope, EventEnvelope, EventId, EventVersion,
};
use crate::KernelError;
use std::future::Future;

pub trait AccountReportEventStore: Sync + Send + 'static {
    type Connection: Connection;

    fn persist(
        &self,
        executor: &mut Self::Connection,
        command: &CommandEnvelope<AccountReportEvent, AccountReport>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn persist_and_transform(
        &self,
        executor: &mut Self::Connection,
        command: CommandEnvelope<AccountReportEvent, AccountReport>,
    ) -> impl Future<
        Output = error_stack::Result<EventEnvelope<AccountReportEvent, AccountReport>, KernelError>,
    > + Send;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &EventId<AccountReportEvent, AccountReport>,
        since: Option<&EventVersion<AccountReport>>,
    ) -> impl Future<
        Output = error_stack::Result<
            Vec<EventEnvelope<AccountReportEvent, AccountReport>>,
            KernelError,
        >,
    > + Send;
}

pub trait DependOnAccountReportEventStore: Sync + Send + DependOnDatabaseConnection {
    type AccountReportEventStore: AccountReportEventStore<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn account_report_event_store(&self) -> &Self::AccountReportEventStore;
}
