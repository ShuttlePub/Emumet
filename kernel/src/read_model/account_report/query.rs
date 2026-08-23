use super::{AccountReportProjection, AccountReportReadModel, DependOnAccountReportReadModel};
use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::AccountReportId;
use crate::KernelError;
use std::future::Future;

pub trait AccountReportQuery: Send + Sync + 'static {
    type Connection: Connection;

    fn find_by_id(
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
}

impl<T> AccountReportQuery for T
where
    T: DependOnAccountReportReadModel + Send + Sync + 'static,
{
    type Connection = <<T as DependOnAccountReportReadModel>::AccountReportReadModel as AccountReportReadModel>::Connection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &AccountReportId,
    ) -> error_stack::Result<Option<AccountReportProjection>, KernelError> {
        self.account_report_read_model()
            .find_by_id(executor, id)
            .await
    }

    async fn find_open(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<Vec<AccountReportProjection>, KernelError> {
        self.account_report_read_model().find_open(executor).await
    }

    async fn find_all(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<Vec<AccountReportProjection>, KernelError> {
        self.account_report_read_model().find_all(executor).await
    }

    async fn count_open(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<i64, KernelError> {
        self.account_report_read_model().count_open(executor).await
    }
}

pub trait DependOnAccountReportQuery: DependOnDatabaseConnection + Send + Sync {
    type AccountReportQuery: AccountReportQuery<
        Connection = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    >;
    fn account_report_query(&self) -> &Self::AccountReportQuery;
}

impl<T> DependOnAccountReportQuery for T
where
    T: DependOnAccountReportReadModel + DependOnDatabaseConnection + Send + Sync + 'static,
{
    type AccountReportQuery = Self;
    fn account_report_query(&self) -> &Self::AccountReportQuery {
        self
    }
}
