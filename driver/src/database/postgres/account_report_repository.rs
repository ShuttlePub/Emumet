use super::account_report_event_store::PostgresAccountReportEventStore;
use crate::database::{PostgresConnection, PostgresDatabase};
use error_stack::Report;
use kernel::interfaces::event_store::AccountReportEventStore;
use kernel::interfaces::repository::{
    AggregateRepository, DependOnAccountReportRepository, Rehydrated,
};
use kernel::prelude::entity::{
    AccountReport, AccountReportEvent, AccountReportId, CommandEnvelope, EventEnvelope, EventId,
};
use kernel::KernelError;

pub struct PostgresAccountReportRepository;

impl AggregateRepository<AccountReport> for PostgresAccountReportRepository {
    type Connection = PostgresConnection;
    type Id = AccountReportId;

    async fn load(
        &self,
        executor: &mut Self::Connection,
        id: &Self::Id,
    ) -> error_stack::Result<Rehydrated<AccountReport>, KernelError> {
        let events = PostgresAccountReportEventStore
            .find_by_id(executor, &EventId::from(id.clone()), None)
            .await?;
        Rehydrated::<AccountReport>::from_events(events)?.ok_or_else(|| {
            Report::new(KernelError::NotFound).attach_printable(format!(
                "No events found for account report: {}",
                id.as_ref()
            ))
        })
    }

    async fn save(
        &self,
        executor: &mut Self::Connection,
        command: CommandEnvelope<AccountReportEvent, AccountReport>,
    ) -> error_stack::Result<EventEnvelope<AccountReportEvent, AccountReport>, KernelError> {
        PostgresAccountReportEventStore
            .persist_and_transform(executor, command)
            .await
    }
}

impl DependOnAccountReportRepository for PostgresDatabase {
    type AccountReportRepository = PostgresAccountReportRepository;

    fn account_report_repository(&self) -> &Self::AccountReportRepository {
        &PostgresAccountReportRepository
    }
}
