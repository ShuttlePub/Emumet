use crate::api::ReportApi;
use crate::auth::{AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::schema::report::{AccountReportResponse, CreateReportRequest};
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use kernel::prelude::entity::ReportCategory;

fn parse_report_category(category: &str) -> Result<ReportCategory, ErrorStatus> {
    match category {
        "spam" => Ok(ReportCategory::Spam),
        "harassment" => Ok(ReportCategory::Harassment),
        "other" => Ok(ReportCategory::Other),
        _ => Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            format!(
                "invalid report category: expected \"spam\", \"harassment\", or \"other\": {category}"
            ),
        ))),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/reports",
    description = "Create an account moderation report.",
    request_body = CreateReportRequest,
    responses(
        (status = 201, description = "Report created", body = AccountReportResponse),
        (status = 400, description = "Invalid report"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 404, description = "Target account not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Report",
)]
pub(crate) async fn create_report(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<ReportApi>,
    Json(request): Json<CreateReportRequest>,
) -> Result<(StatusCode, Json<AccountReportResponse>), ErrorStatus> {
    let category = parse_report_category(&request.category)?;
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    let report = api
        .create_report(&auth_account_id, request.target, category, request.comment)
        .await
        .map_err(ErrorStatus::from)?;

    Ok((StatusCode::CREATED, Json((&report).into())))
}
