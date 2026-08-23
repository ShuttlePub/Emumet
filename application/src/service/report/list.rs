use crate::permission::{check_permission, instance_moderate};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::read_model::{
    AccountReportProjection, AccountReportQuery, DependOnAccountReportQuery,
};
use kernel::prelude::entity::AuthAccountId;
use kernel::KernelError;
use std::future::Future;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReportList {
    reports: Vec<AccountReportProjection>,
    open_count: i64,
}

impl ReportList {
    pub fn new(reports: Vec<AccountReportProjection>, open_count: i64) -> Self {
        Self {
            reports,
            open_count,
        }
    }

    pub fn reports(&self) -> &[AccountReportProjection] {
        &self.reports
    }

    pub fn open_count(&self) -> i64 {
        self.open_count
    }
}

pub trait ListReportsUseCase:
    'static + Sync + Send + DependOnAccountReportQuery + DependOnPermissionChecker
{
    fn list_reports<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<ReportList, KernelError>> + Send + 'a {
        async move {
            check_permission(self, auth_account_id, &instance_moderate()).await?;
            let mut connection = self.database_connection().connection().await?;
            let reports = self
                .account_report_query()
                .find_open(&mut connection)
                .await?;
            let open_count = self
                .account_report_query()
                .count_open(&mut connection)
                .await?;
            Ok(ReportList::new(reports, open_count))
        }
    }
}

impl<T> ListReportsUseCase for T where
    T: 'static + DependOnAccountReportQuery + DependOnPermissionChecker
{
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{fixture, projection};
    use super::*;
    use kernel::KernelError;

    #[tokio::test]
    async fn list_reports_returns_open_reports_and_open_count_for_moderator() {
        let fixture = fixture(
            false,
            false,
            true,
            None,
            vec![projection(1, false), projection(2, true)],
        );

        let result = fixture
            .module
            .list_reports(&fixture.operator_id)
            .await
            .unwrap();

        assert_eq!(result.reports().len(), 1);
        assert_eq!(result.open_count(), 1);
    }

    #[tokio::test]
    async fn list_reports_rejects_non_moderator() {
        let fixture = fixture(false, false, false, None, vec![projection(1, false)]);

        let result = fixture.module.list_reports(&fixture.operator_id).await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::PermissionDenied
        );
    }
}
