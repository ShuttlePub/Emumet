mod query;

pub use self::query::*;

use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    AccountId, AccountReport, AccountReportId, CloseReason, EventVersion, Nanoid, ReportCategory,
    ReportComment, ReportResolution, ReportStatus,
};
use crate::KernelError;
use std::future::Future;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AccountReportProjection {
    id: AccountReportId,
    target: AccountId,
    reported_by: AccountId,
    category: ReportCategory,
    comment: Option<ReportComment>,
    status: ReportStatus,
    resolution: Option<ReportResolution>,
    close_reason: Option<CloseReason>,
    version: EventVersion<AccountReport>,
    nanoid: Nanoid<AccountReport>,
}

impl AccountReportProjection {
    pub fn new(
        id: AccountReportId,
        target: AccountId,
        reported_by: AccountId,
        category: ReportCategory,
        comment: Option<ReportComment>,
        status: ReportStatus,
        resolution: Option<ReportResolution>,
        close_reason: Option<CloseReason>,
        version: EventVersion<AccountReport>,
        nanoid: Nanoid<AccountReport>,
    ) -> Self {
        Self {
            id,
            target,
            reported_by,
            category,
            comment,
            status,
            resolution,
            close_reason,
            version,
            nanoid,
        }
    }

    pub fn id(&self) -> &AccountReportId {
        &self.id
    }

    pub fn target(&self) -> &AccountId {
        &self.target
    }

    pub fn reported_by(&self) -> &AccountId {
        &self.reported_by
    }

    pub fn category(&self) -> &ReportCategory {
        &self.category
    }

    pub fn comment(&self) -> &Option<ReportComment> {
        &self.comment
    }

    pub fn status(&self) -> &ReportStatus {
        &self.status
    }

    pub fn resolution(&self) -> &Option<ReportResolution> {
        &self.resolution
    }

    pub fn close_reason(&self) -> &Option<CloseReason> {
        &self.close_reason
    }

    pub fn version(&self) -> &EventVersion<AccountReport> {
        &self.version
    }

    pub fn nanoid(&self) -> &Nanoid<AccountReport> {
        &self.nanoid
    }
}

impl From<AccountReport> for AccountReportProjection {
    fn from(value: AccountReport) -> Self {
        let destruct = value.into_destruct();
        let (resolution, close_reason) = match &destruct.status {
            ReportStatus::Open => (None, None),
            ReportStatus::Closed {
                resolution,
                close_reason,
            } => (Some(resolution.clone()), Some(close_reason.clone())),
        };
        Self::new(
            destruct.id,
            destruct.target,
            destruct.reported_by,
            destruct.category,
            destruct.comment,
            destruct.status,
            resolution,
            close_reason,
            destruct.version,
            destruct.nanoid,
        )
    }
}

impl From<AccountReportProjection> for AccountReport {
    fn from(value: AccountReportProjection) -> Self {
        AccountReport::reconstitute(
            value.id().clone(),
            value.target().clone(),
            value.reported_by().clone(),
            value.category().clone(),
            value.comment().clone(),
            value.status().clone(),
            value.version().clone(),
            value.nanoid().clone(),
        )
    }
}

pub trait AccountReportReadModel: Sync + Send + 'static {
    type Connection: Connection;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &AccountReportId,
    ) -> impl Future<Output = error_stack::Result<Option<AccountReportProjection>, KernelError>> + Send;

    fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Connection,
        id: &AccountReportId,
    ) -> impl Future<Output = error_stack::Result<Option<AccountReportProjection>, KernelError>> + Send;

    /// Return open reports in reverse chronological order.
    fn find_open(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<Vec<AccountReportProjection>, KernelError>> + Send;

    /// Return all reports in reverse chronological order.
    fn find_all(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<Vec<AccountReportProjection>, KernelError>> + Send;

    fn count_open(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<i64, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Connection,
        account_report: &AccountReport,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Connection,
        account_report: &AccountReport,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnAccountReportReadModel: Sync + Send + DependOnDatabaseConnection {
    type AccountReportReadModel: AccountReportReadModel<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn account_report_read_model(&self) -> &Self::AccountReportReadModel;
}
