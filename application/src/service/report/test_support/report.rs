use super::database::MockConnection;
use error_stack::Report;
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{AccountReportProjection, AccountReportReadModel};
use kernel::interfaces::repository::{AggregateRepository, Rehydrated};
use kernel::prelude::entity::{
    AccountId, AccountReport, AccountReportEvent, AccountReportId, CloseReason, CommandEnvelope,
    EventEnvelope, EventVersion, Nanoid, ReportCategory, ReportResolution,
};
use kernel::KernelError;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct MockAccountReportRepository {
    pub report: Option<AccountReport>,
    pub saved_events: Arc<Mutex<Vec<AccountReportEvent>>>,
}

impl AggregateRepository<AccountReport> for MockAccountReportRepository {
    type Connection = MockConnection;
    type Id = AccountReportId;

    async fn load(
        &self,
        _executor: &mut Self::Connection,
        id: &Self::Id,
    ) -> error_stack::Result<Rehydrated<AccountReport>, KernelError> {
        self.report
            .as_ref()
            .filter(|report| report.id() == id)
            .map(|report| Rehydrated::new(report.clone(), report.version().clone()))
            .ok_or_else(|| Report::new(KernelError::NotFound))
    }

    async fn save(
        &self,
        _executor: &mut Self::Connection,
        command: CommandEnvelope<AccountReportEvent, AccountReport>,
    ) -> error_stack::Result<EventEnvelope<AccountReportEvent, AccountReport>, KernelError> {
        self.saved_events
            .lock()
            .unwrap()
            .push(command.event().clone());
        Ok(EventEnvelope::new(
            command.id().clone(),
            command.event().clone(),
            EventVersion::new(2),
        ))
    }
}

#[derive(Clone)]
pub struct MockAccountReportReadModel {
    pub reports: Arc<Mutex<Vec<AccountReportProjection>>>,
}

impl AccountReportReadModel for MockAccountReportReadModel {
    type Connection = MockConnection;

    async fn find_by_id(
        &self,
        _executor: &mut Self::Connection,
        id: &AccountReportId,
    ) -> error_stack::Result<Option<AccountReportProjection>, KernelError> {
        Ok(self
            .reports
            .lock()
            .unwrap()
            .iter()
            .find(|report| report.id() == id)
            .cloned())
    }

    async fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Connection,
        id: &AccountReportId,
    ) -> error_stack::Result<Option<AccountReportProjection>, KernelError> {
        self.find_by_id(executor, id).await
    }

    async fn find_open(
        &self,
        _executor: &mut Self::Connection,
    ) -> error_stack::Result<Vec<AccountReportProjection>, KernelError> {
        Ok(self
            .reports
            .lock()
            .unwrap()
            .iter()
            .filter(|report| !report.status().is_closed())
            .cloned()
            .collect())
    }

    async fn find_all(
        &self,
        _executor: &mut Self::Connection,
    ) -> error_stack::Result<Vec<AccountReportProjection>, KernelError> {
        Ok(self.reports.lock().unwrap().clone())
    }

    async fn count_open(
        &self,
        _executor: &mut Self::Connection,
    ) -> error_stack::Result<i64, KernelError> {
        Ok(self
            .reports
            .lock()
            .unwrap()
            .iter()
            .filter(|report| !report.status().is_closed())
            .count() as i64)
    }

    async fn create(
        &self,
        _executor: &mut Self::Connection,
        report: &AccountReport,
    ) -> error_stack::Result<(), KernelError> {
        self.reports.lock().unwrap().push(report.clone().into());
        Ok(())
    }

    async fn update(
        &self,
        _executor: &mut Self::Connection,
        report: &AccountReport,
    ) -> error_stack::Result<(), KernelError> {
        let mut reports = self.reports.lock().unwrap();
        if let Some(existing) = reports.iter_mut().find(|item| item.id() == report.id()) {
            *existing = report.clone().into();
        }
        Ok(())
    }
}

pub fn open_report(id: i64) -> AccountReport {
    kernel::ensure_generator_initialized();
    let report_id = AccountReportId::new(id);
    let command = AccountReport::create(
        report_id,
        AccountId::new(20),
        AccountId::new(10),
        ReportCategory::Spam,
        None,
        Nanoid::new(format!("report-{id}")),
    );
    let mut report = None;
    AccountReport::apply(
        &mut report,
        EventEnvelope::new(
            command.id().clone(),
            command.event().clone(),
            EventVersion::new(1),
        ),
    )
    .unwrap();
    report.unwrap()
}

pub fn projection(id: i64, closed: bool) -> AccountReportProjection {
    let mut report = open_report(id);
    if closed {
        let mut entity = Some(report);
        AccountReport::apply(
            &mut entity,
            EventEnvelope::new(
                AccountReportId::new(id).into(),
                AccountReportEvent::Closed {
                    resolution: ReportResolution::Dismissed,
                    close_reason: CloseReason::new("not actionable"),
                },
                EventVersion::new(2),
            ),
        )
        .unwrap();
        report = entity.unwrap();
    }
    report.into()
}
