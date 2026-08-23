mod create;
mod dismiss;
mod list;
mod resolve;

pub use create::CreateReportUseCase;
pub use dismiss::DismissReportUseCase;
pub use list::{ListReportsUseCase, ReportList};
pub use resolve::ResolveReportUseCase;

use crate::permission::{check_permission, instance_moderate};
use error_stack::Report;
use kernel::interfaces::database::{DependOnTransactionManager, TransactionManager};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::read_model::{AccountReportReadModel, DependOnAccountReportReadModel};
use kernel::interfaces::repository::{AggregateRepository, DependOnAccountReportRepository};
use kernel::prelude::entity::{
    AccountReport, AccountReportId, AuthAccountId, CloseReason, ReportResolution,
};
use kernel::KernelError;

struct CloseReportCommand<'a> {
    auth_account_id: &'a AuthAccountId,
    report_id: AccountReportId,
    resolution: ReportResolution,
    reason: String,
}

async fn close_report<T>(
    deps: &T,
    command: CloseReportCommand<'_>,
) -> error_stack::Result<(), KernelError>
where
    T: 'static
        + Clone
        + DependOnAccountReportRepository
        + DependOnAccountReportReadModel
        + DependOnTransactionManager
        + DependOnPermissionChecker,
{
    let reason = CloseReason::new(command.reason);
    reason.validate()?;
    check_permission(deps, command.auth_account_id, &instance_moderate()).await?;

    let report_id = command.report_id;
    let resolution = command.resolution;
    let transaction_deps = deps.clone();
    deps.transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                let (report, current_version) = transaction_deps
                    .account_report_repository()
                    .load(executor, &report_id)
                    .await?
                    .into_parts();
                if report.status().is_closed() {
                    return Err(Report::new(KernelError::Rejected)
                        .attach_printable("Account report is already closed"));
                }

                let event = transaction_deps
                    .account_report_repository()
                    .save(
                        executor,
                        AccountReport::close(report_id, resolution, reason, current_version),
                    )
                    .await?;
                let mut updated = Some(report);
                AccountReport::apply(&mut updated, event)?;
                let updated = updated.ok_or_else(|| {
                    Report::new(KernelError::Internal)
                        .attach_printable("Failed to construct closed account report")
                })?;
                transaction_deps
                    .account_report_read_model()
                    .update(executor, &updated)
                    .await?;
                Ok(())
            })
        })
        .await
}

#[cfg(test)]
mod test_support;
