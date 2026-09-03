use crate::api::AdminReportApi;
use crate::auth::{AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::schema::report::{AccountReportListResponse, CloseReportRequest};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use kernel::prelude::entity::AccountReportId;

fn parse_report_id(id: &str) -> Result<AccountReportId, ErrorStatus> {
    id.parse::<i64>().map(AccountReportId::new).map_err(|_| {
        ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            format!("invalid report id: expected an integer: {id}"),
        ))
    })
}

fn parse_close_request(
    request: Result<Json<CloseReportRequest>, JsonRejection>,
) -> Result<CloseReportRequest, ErrorStatus> {
    request
        .map(|Json(request)| request)
        .map_err(|rejection| ErrorStatus::from((StatusCode::BAD_REQUEST, rejection.body_text())))
}

fn is_warned_resolution(resolution: Option<&str>) -> Result<bool, ErrorStatus> {
    match resolution {
        None | Some("resolved") => Ok(false),
        Some("warned") => Ok(true),
        Some(value) => Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            format!("invalid resolution: expected resolved or warned: {value}"),
        ))),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/reports",
    description = "List open account reports for the moderator queue.",
    responses(
        (status = 200, description = "Open reports", body = AccountReportListResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
    ),
    security(("bearer_auth" = [])),
    tag = "Report",
)]
pub(crate) async fn list_reports(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminReportApi>,
) -> Result<Json<AccountReportListResponse>, ErrorStatus> {
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    let reports = api
        .list_reports(&auth_account_id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(Json(AccountReportListResponse {
        reports: reports.reports().iter().map(Into::into).collect(),
        open_count: reports.open_count(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/reports/{id}/resolve",
    description = "Resolve an open account report.",
    params(("id" = i64, Path, description = "Account report ID")),
    request_body = CloseReportRequest,
    responses(
        (status = 204, description = "Report resolved"),
        (status = 400, description = "Invalid report ID or close reason"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "Report not found"),
        (status = 422, description = "Report already closed"),
    ),
    security(("bearer_auth" = [])),
    tag = "Report",
)]
pub(crate) async fn resolve_report(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminReportApi>,
    Path(id): Path<String>,
    request: Result<Json<CloseReportRequest>, JsonRejection>,
) -> Result<StatusCode, ErrorStatus> {
    let report_id = parse_report_id(&id)?;
    let request = parse_close_request(request)?;
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    if is_warned_resolution(request.resolution.as_deref())? {
        api.warn_report(&auth_account_id, report_id, request.close_reason)
            .await
            .map_err(ErrorStatus::from)?;
    } else {
        api.resolve_report(&auth_account_id, report_id, request.close_reason)
            .await
            .map_err(ErrorStatus::from)?;
    }

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/reports/{id}/dismiss",
    description = "Dismiss an open account report.",
    params(("id" = i64, Path, description = "Account report ID")),
    request_body = CloseReportRequest,
    responses(
        (status = 204, description = "Report dismissed"),
        (status = 400, description = "Invalid report ID or close reason"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "Report not found"),
        (status = 422, description = "Report already closed"),
    ),
    security(("bearer_auth" = [])),
    tag = "Report",
)]
pub(crate) async fn dismiss_report(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AdminReportApi>,
    Path(id): Path<String>,
    request: Result<Json<CloseReportRequest>, JsonRejection>,
) -> Result<StatusCode, ErrorStatus> {
    let report_id = parse_report_id(&id)?;
    let request = parse_close_request(request)?;
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    api.dismiss_report(&auth_account_id, report_id, request.close_reason)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}
