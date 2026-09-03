use kernel::interfaces::read_model::AccountReportProjection;
use kernel::prelude::entity::{ReportCategory, ReportResolution, ReportStatus};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct CreateReportRequest {
    pub target: String,
    pub category: String,
    pub comment: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct CloseReportRequest {
    pub close_reason: String,
    #[serde(default)]
    pub resolution: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct AccountReportResponse {
    pub id: i64,
    pub nanoid: String,
    pub target: i64,
    pub reported_by: i64,
    pub category: String,
    pub comment: Option<String>,
    pub status: String,
    pub resolution: Option<String>,
    pub close_reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct AccountReportListResponse {
    pub reports: Vec<AccountReportResponse>,
    pub open_count: i64,
}

impl From<&AccountReportProjection> for AccountReportResponse {
    fn from(report: &AccountReportProjection) -> Self {
        Self {
            id: *report.id().as_ref(),
            nanoid: report.nanoid().as_ref().to_string(),
            target: *report.target().as_ref(),
            reported_by: *report.reported_by().as_ref(),
            category: report_category_value(report.category()).to_string(),
            comment: report
                .comment()
                .as_ref()
                .map(|comment| comment.as_ref().to_string()),
            status: report_status_value(report.status()).to_string(),
            resolution: report
                .resolution()
                .as_ref()
                .map(|resolution| report_resolution_value(resolution).to_string()),
            close_reason: report
                .close_reason()
                .as_ref()
                .map(|reason| reason.as_ref().to_string()),
        }
    }
}

fn report_category_value(category: &ReportCategory) -> &'static str {
    match category {
        ReportCategory::Spam => "spam",
        ReportCategory::Harassment => "harassment",
        ReportCategory::Other => "other",
    }
}

fn report_status_value(status: &ReportStatus) -> &'static str {
    match status {
        ReportStatus::Open => "open",
        ReportStatus::Closed { .. } => "closed",
    }
}

fn report_resolution_value(resolution: &ReportResolution) -> &'static str {
    match resolution {
        ReportResolution::Resolved => "resolved",
        ReportResolution::Dismissed => "dismissed",
        ReportResolution::Warned => "warned",
    }
}
