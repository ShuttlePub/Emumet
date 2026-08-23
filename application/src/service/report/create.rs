use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{
    AccountQuery, AccountReportProjection, AccountReportReadModel, DependOnAccountQuery,
    DependOnAccountReportReadModel,
};
use kernel::interfaces::repository::{AggregateRepository, DependOnAccountReportRepository};
use kernel::prelude::entity::{
    Account, AccountReport, AccountReportId, AuthAccountId, Nanoid, ReportCategory, ReportComment,
};
use kernel::KernelError;
use std::future::Future;

pub trait CreateReportUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnAccountReportRepository
    + DependOnAccountReportReadModel
    + DependOnTransactionManager
{
    fn create_report<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        target_nanoid: String,
        category: ReportCategory,
        comment: Option<String>,
    ) -> impl Future<Output = error_stack::Result<AccountReportProjection, KernelError>> + Send + 'a
    {
        async move {
            let comment = comment.map(ReportComment::new);
            if let Some(comment) = &comment {
                comment.validate()?;
            }
            match (&category, &comment) {
                (ReportCategory::Other, None) => {
                    return Err(Report::new(KernelError::Validation)
                        .attach_printable("A comment is required for report category Other"));
                }
                (ReportCategory::Spam | ReportCategory::Harassment, Some(_) | None)
                | (ReportCategory::Other, Some(_)) => {}
            }

            let mut connection = self.database_connection().connection().await?;
            let reporter = self
                .account_query()
                .find_by_auth_id(&mut connection, auth_account_id)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable("No account belongs to the authenticated user")
                })?;
            let target_nanoid = Nanoid::<Account>::new(target_nanoid);
            let target = self
                .account_query()
                .find_by_nanoid_unfiltered(&mut connection, &target_nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        target_nanoid.as_ref()
                    ))
                })?;

            let transaction_deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let command = AccountReport::create(
                            AccountReportId::new(kernel::generate_id()),
                            target.id().clone(),
                            reporter.id().clone(),
                            category,
                            comment,
                            Nanoid::default(),
                        );
                        let event = transaction_deps
                            .account_report_repository()
                            .save(executor, command)
                            .await?;
                        let mut report = None;
                        AccountReport::apply(&mut report, event)?;
                        let report = report.ok_or_else(|| {
                            Report::new(KernelError::Internal)
                                .attach_printable("Failed to construct created account report")
                        })?;
                        transaction_deps
                            .account_report_read_model()
                            .create(executor, &report)
                            .await?;
                        Ok(report.into())
                    })
                })
                .await
        }
    }
}

impl<T> CreateReportUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountQuery
        + DependOnAccountReportRepository
        + DependOnAccountReportReadModel
        + DependOnTransactionManager
{
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{fixture, saved_events};
    use super::*;
    use kernel::prelude::entity::{AccountReportEvent, ReportCategory};
    use kernel::KernelError;

    #[tokio::test]
    async fn create_report_saves_created_event_for_owned_reporter_and_existing_target() {
        let fixture = fixture(true, true, true, None, Vec::new());

        let report = fixture
            .module
            .create_report(
                &fixture.operator_id,
                fixture.target_nanoid.clone(),
                ReportCategory::Harassment,
                Some("abusive messages".to_string()),
            )
            .await
            .unwrap();

        assert_eq!(report.target().as_ref(), &20);
        assert!(matches!(
            saved_events(&fixture).as_slice(),
            [AccountReportEvent::Created {
                category: ReportCategory::Harassment,
                ..
            }]
        ));
    }

    #[tokio::test]
    async fn create_report_rejects_other_without_comment_without_saving_event() {
        let fixture = fixture(true, true, true, None, Vec::new());

        let result = fixture
            .module
            .create_report(
                &fixture.operator_id,
                fixture.target_nanoid.clone(),
                ReportCategory::Other,
                None,
            )
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::Validation
        );
        assert!(saved_events(&fixture).is_empty());
    }

    #[tokio::test]
    async fn create_report_returns_not_found_for_unknown_target_without_saving_event() {
        let fixture = fixture(true, false, true, None, Vec::new());

        let result = fixture
            .module
            .create_report(
                &fixture.operator_id,
                fixture.target_nanoid.clone(),
                ReportCategory::Spam,
                None,
            )
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::NotFound
        );
        assert!(saved_events(&fixture).is_empty());
    }

    #[tokio::test]
    async fn create_report_rejects_invalid_comment_without_saving_event() {
        let fixture = fixture(true, true, true, None, Vec::new());

        let result = fixture
            .module
            .create_report(
                &fixture.operator_id,
                fixture.target_nanoid.clone(),
                ReportCategory::Spam,
                Some(String::new()),
            )
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::Validation
        );
        assert!(saved_events(&fixture).is_empty());
    }

    #[tokio::test]
    async fn create_report_rejects_comment_over_limit_without_saving_event() {
        let fixture = fixture(true, true, true, None, Vec::new());

        let result = fixture
            .module
            .create_report(
                &fixture.operator_id,
                fixture.target_nanoid.clone(),
                ReportCategory::Spam,
                Some("x".repeat(501)),
            )
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::Validation
        );
        assert!(saved_events(&fixture).is_empty());
    }
}
