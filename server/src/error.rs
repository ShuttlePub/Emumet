use axum::http::StatusCode;
use axum::response::IntoResponse;
use error_stack::Report;
use kernel::KernelError;
use std::process::{ExitCode, Termination};

#[derive(Debug)]
pub struct StackTrace(Report<KernelError>);

impl From<Report<KernelError>> for StackTrace {
    fn from(e: Report<KernelError>) -> Self {
        StackTrace(e)
    }
}

impl Termination for StackTrace {
    fn report(self) -> ExitCode {
        self.0.report()
    }
}

#[derive(Debug)]
pub enum ErrorStatus {
    Report(Report<KernelError>),
    StatusCode(StatusCode),
    StatusCodeWithMessage(StatusCode, String),
}

impl From<Report<KernelError>> for ErrorStatus {
    fn from(e: Report<KernelError>) -> Self {
        ErrorStatus::Report(e)
    }
}

impl From<StatusCode> for ErrorStatus {
    fn from(code: StatusCode) -> Self {
        ErrorStatus::StatusCode(code)
    }
}

impl From<(StatusCode, String)> for ErrorStatus {
    fn from((code, message): (StatusCode, String)) -> Self {
        ErrorStatus::StatusCodeWithMessage(code, message)
    }
}

impl IntoResponse for ErrorStatus {
    fn into_response(self) -> axum::response::Response {
        match self {
            ErrorStatus::Report(e) => match e.current_context() {
                KernelError::Concurrency => StatusCode::CONFLICT,
                KernelError::Timeout => StatusCode::REQUEST_TIMEOUT,
                KernelError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
                KernelError::PermissionDenied => StatusCode::FORBIDDEN,
                KernelError::NotFound => StatusCode::NOT_FOUND,
                KernelError::Rejected => StatusCode::UNPROCESSABLE_ENTITY,
            }
            .into_response(),
            ErrorStatus::StatusCode(code) => code.into_response(),
            ErrorStatus::StatusCodeWithMessage(code, message) => (code, message).into_response(),
        }
    }
}
