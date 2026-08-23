use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::read_model::{
    AccountReportProjection, AccountReportReadModel, DependOnAccountReportReadModel,
};
use kernel::prelude::entity::{
    AccountId, AccountReport, AccountReportId, CloseReason, EventVersion, Nanoid, ReportCategory,
    ReportComment, ReportResolution, ReportStatus,
};
use kernel::KernelError;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct AccountReportRow {
    id: i64,
    target_account_id: i64,
    reported_by_account_id: i64,
    category: String,
    comment: Option<String>,
    status: String,
    resolution: Option<String>,
    close_reason: Option<String>,
    version: i64,
    nanoid: String,
}

impl TryFrom<AccountReportRow> for AccountReportProjection {
    type Error = Report<KernelError>;

    fn try_from(value: AccountReportRow) -> Result<Self, Self::Error> {
        let category = match value.category.as_str() {
            "spam" => ReportCategory::Spam,
            "harassment" => ReportCategory::Harassment,
            "other" => ReportCategory::Other,
            category => {
                return Err(Report::new(KernelError::Internal)
                    .attach_printable(format!("Unknown account report category: {category}")))
            }
        };
        let closed = match (
            value.status.as_str(),
            value.resolution.as_deref(),
            value.close_reason,
        ) {
            ("open", None, None) => None,
            ("resolved", Some("resolved"), Some(reason)) => {
                Some((ReportResolution::Resolved, CloseReason::new(reason)))
            }
            ("dismissed", Some("dismissed"), Some(reason)) => {
                Some((ReportResolution::Dismissed, CloseReason::new(reason)))
            }
            (status, resolution, close_reason) => {
                return Err(Report::new(KernelError::Internal).attach_printable(format!(
                    "Invalid account report status fields: status={status}, resolution={resolution:?}, close_reason={close_reason:?}"
                )))
            }
        };
        let (status, resolution, close_reason) = match closed {
            None => (ReportStatus::Open, None, None),
            Some((resolution, close_reason)) => (
                ReportStatus::Closed {
                    resolution: resolution.clone(),
                    close_reason: close_reason.clone(),
                },
                Some(resolution),
                Some(close_reason),
            ),
        };

        Ok(AccountReportProjection::new(
            AccountReportId::new(value.id),
            AccountId::new(value.target_account_id),
            AccountId::new(value.reported_by_account_id),
            category,
            value.comment.map(ReportComment::new),
            status,
            resolution,
            close_reason,
            EventVersion::new(value.version),
            Nanoid::new(value.nanoid),
        ))
    }
}

pub struct PostgresAccountReportReadModel;

impl AccountReportReadModel for PostgresAccountReportReadModel {
    type Connection = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &AccountReportId,
    ) -> error_stack::Result<Option<AccountReportProjection>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AccountReportRow>(
            //language=postgresql
            r#"
            SELECT id, target_account_id, reported_by_account_id, category, comment, status, resolution, close_reason, version, nanoid
            FROM account_reports WHERE id = $1
            "#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(TryFrom::try_from)
        .transpose()
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
        executor: &mut Self::Connection,
    ) -> error_stack::Result<Vec<AccountReportProjection>, KernelError> {
        let con: &mut PgConnection = executor;
        let rows = sqlx::query_as::<_, AccountReportRow>(
            //language=postgresql
            r#"
            SELECT id, target_account_id, reported_by_account_id, category, comment, status, resolution, close_reason, version, nanoid
            FROM account_reports WHERE status = 'open'
            ORDER BY id DESC
            "#,
        )
        .fetch_all(con)
        .await
        .convert_error()?;
        rows.into_iter()
            .map(TryFrom::try_from)
            .collect::<error_stack::Result<Vec<_>, KernelError>>()
    }

    async fn find_all(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<Vec<AccountReportProjection>, KernelError> {
        let con: &mut PgConnection = executor;
        let rows = sqlx::query_as::<_, AccountReportRow>(
            //language=postgresql
            r#"
            SELECT id, target_account_id, reported_by_account_id, category, comment, status, resolution, close_reason, version, nanoid
            FROM account_reports
            ORDER BY id DESC
            "#,
        )
        .fetch_all(con)
        .await
        .convert_error()?;
        rows.into_iter()
            .map(TryFrom::try_from)
            .collect::<error_stack::Result<Vec<_>, KernelError>>()
    }

    async fn count_open(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<i64, KernelError> {
        let con: &mut PgConnection = executor;
        let row: (i64,) = sqlx::query_as(
            //language=postgresql
            "SELECT COUNT(*) FROM account_reports WHERE status = 'open'",
        )
        .fetch_one(con)
        .await
        .convert_error()?;
        Ok(row.0)
    }

    async fn create(
        &self,
        executor: &mut Self::Connection,
        account_report: &AccountReport,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let (status, resolution, close_reason) = report_status_fields(account_report.status());
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO account_reports (id, target_account_id, reported_by_account_id, category,
                comment, status, resolution, close_reason, version, nanoid)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(account_report.id().as_ref())
        .bind(account_report.target().as_ref())
        .bind(account_report.reported_by().as_ref())
        .bind(match account_report.category() {
            ReportCategory::Spam => "spam",
            ReportCategory::Harassment => "harassment",
            ReportCategory::Other => "other",
        })
        .bind(account_report.comment().as_ref().map(ReportComment::as_ref))
        .bind(status)
        .bind(resolution)
        .bind(close_reason)
        .bind(account_report.version().as_ref())
        .bind(account_report.nanoid().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn update(
        &self,
        executor: &mut Self::Connection,
        account_report: &AccountReport,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let (status, resolution, close_reason) = report_status_fields(account_report.status());
        let result = sqlx::query(
            //language=postgresql
            r#"
            UPDATE account_reports
            SET status = $2, resolution = $3, close_reason = $4, version = $5
            WHERE id = $1
            "#,
        )
        .bind(account_report.id().as_ref())
        .bind(status)
        .bind(resolution)
        .bind(close_reason)
        .bind(account_report.version().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound)
                .attach_printable("Target account report not found for update"));
        }
        Ok(())
    }
}

fn report_status_fields(
    status: &ReportStatus,
) -> (&'static str, Option<&'static str>, Option<&str>) {
    match status {
        ReportStatus::Open => ("open", None, None),
        ReportStatus::Closed {
            resolution,
            close_reason,
        } => {
            let resolution = report_resolution_value(resolution);
            (resolution, Some(resolution), Some(close_reason.as_ref()))
        }
    }
}

fn report_resolution_value(resolution: &ReportResolution) -> &'static str {
    match resolution {
        ReportResolution::Resolved => "resolved",
        ReportResolution::Dismissed => "dismissed",
    }
}

impl DependOnAccountReportReadModel for PostgresDatabase {
    type AccountReportReadModel = PostgresAccountReportReadModel;

    fn account_report_read_model(&self) -> &Self::AccountReportReadModel {
        &PostgresAccountReportReadModel
    }
}
