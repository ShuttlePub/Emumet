use super::{close_report, CloseReportCommand};
use kernel::interfaces::database::DependOnTransactionManager;
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::read_model::DependOnAccountReportReadModel;
use kernel::interfaces::repository::DependOnAccountReportRepository;
use kernel::prelude::entity::{AccountReportId, AuthAccountId, ReportResolution};
use kernel::KernelError;
use std::future::Future;

pub trait DismissReportUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountReportRepository
    + DependOnAccountReportReadModel
    + DependOnTransactionManager
    + DependOnPermissionChecker
{
    fn dismiss_report<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        report_id: AccountReportId,
        reason: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        close_report(
            self,
            CloseReportCommand {
                auth_account_id,
                report_id,
                resolution: ReportResolution::Dismissed,
                reason,
            },
        )
    }
}

impl<T> DismissReportUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountReportRepository
        + DependOnAccountReportReadModel
        + DependOnTransactionManager
        + DependOnPermissionChecker
{
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{fixture, open_report, projection, saved_events};
    use super::*;
    use kernel::prelude::entity::{
        AccountReport, AccountReportEvent, AccountReportId, CloseReason, ReportResolution,
    };
    use kernel::KernelError;

    #[tokio::test]
    async fn dismiss_report_saves_dismissed_event_for_open_report() {
        let fixture = fixture(false, false, true, Some(open_report(1)), Vec::new());

        fixture
            .module
            .dismiss_report(
                &fixture.operator_id,
                AccountReportId::new(1),
                "insufficient evidence".to_string(),
            )
            .await
            .unwrap();

        assert_eq!(
            saved_events(&fixture),
            vec![AccountReportEvent::Closed {
                resolution: ReportResolution::Dismissed,
                close_reason: CloseReason::new("insufficient evidence"),
            }]
        );
    }

    #[tokio::test]
    async fn dismiss_report_rejects_non_moderator_without_saving_event() {
        let fixture = fixture(false, false, false, Some(open_report(1)), Vec::new());

        let result = fixture
            .module
            .dismiss_report(
                &fixture.operator_id,
                AccountReportId::new(1),
                "reason".to_string(),
            )
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::PermissionDenied
        );
        assert!(saved_events(&fixture).is_empty());
    }

    #[tokio::test]
    async fn dismiss_report_rejects_closed_report_without_saving_event() {
        let closed: AccountReport = projection(1, true).into();
        let fixture = fixture(false, false, true, Some(closed), Vec::new());

        let result = fixture
            .module
            .dismiss_report(
                &fixture.operator_id,
                AccountReportId::new(1),
                "reason".to_string(),
            )
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::Rejected
        );
        assert!(saved_events(&fixture).is_empty());
    }

    #[tokio::test]
    async fn dismiss_report_returns_not_found_for_unknown_report() {
        let fixture = fixture(false, false, true, None, Vec::new());

        let result = fixture
            .module
            .dismiss_report(
                &fixture.operator_id,
                AccountReportId::new(1),
                "reason".to_string(),
            )
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::NotFound
        );
        assert!(saved_events(&fixture).is_empty());
    }
}
