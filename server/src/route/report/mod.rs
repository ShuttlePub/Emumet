mod admin;
mod client;

pub(crate) use admin::{
    __path_dismiss_report, __path_list_reports, __path_resolve_report, dismiss_report,
    list_reports, resolve_report,
};
pub(crate) use client::{__path_create_report, create_report};

use crate::handler::AppModule;
use axum::routing::{get, post};
use axum::Router;

pub trait ReportRouter {
    fn route_reports(self) -> Self;
}

pub trait AdminReportRouter {
    fn route_admin_reports(self) -> Self;
}

impl ReportRouter for Router<AppModule> {
    fn route_reports(self) -> Self {
        self.route("/reports", post(create_report))
    }
}

impl AdminReportRouter for Router<AppModule> {
    fn route_admin_reports(self) -> Self {
        self.route("/reports", get(list_reports))
            .route("/reports/{id}/resolve", post(resolve_report))
            .route("/reports/{id}/dismiss", post(dismiss_report))
    }
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
