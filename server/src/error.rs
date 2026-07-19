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
                KernelError::Concurrency => {
                    tracing::warn!("Concurrency conflict: {e:?}");
                    StatusCode::CONFLICT.into_response()
                }
                KernelError::Timeout => {
                    tracing::warn!("Request timeout: {e:?}");
                    StatusCode::REQUEST_TIMEOUT.into_response()
                }
                KernelError::Internal => {
                    tracing::error!("Internal error: {e:?}");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
                KernelError::PermissionDenied => StatusCode::FORBIDDEN.into_response(),
                KernelError::NotFound => StatusCode::NOT_FOUND.into_response(),
                KernelError::Rejected => {
                    tracing::warn!("Request rejected: {e:?}");
                    StatusCode::UNPROCESSABLE_ENTITY.into_response()
                }
                KernelError::Validation => {
                    tracing::warn!("Validation failed: {e:?}");
                    let message = e
                        .frames()
                        .find_map(|frame| frame.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| KernelError::Validation.to_string());
                    (StatusCode::BAD_REQUEST, message).into_response()
                }
            },
            ErrorStatus::StatusCode(code) => code.into_response(),
            ErrorStatus::StatusCodeWithMessage(code, message) => (code, message).into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn response_parts(error: ErrorStatus) -> (StatusCode, String) {
        let response = error.into_response();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn validation_error_maps_to_400_with_message() {
        let report = Report::new(KernelError::Validation)
            .attach_printable("Display name must not exceed 100 characters".to_string());

        let (status, body) = response_parts(ErrorStatus::from(report)).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, "Display name must not exceed 100 characters");
    }

    #[tokio::test]
    async fn validation_error_without_message_maps_to_400_with_fallback() {
        let (status, body) =
            response_parts(ErrorStatus::from(Report::new(KernelError::Validation))).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, "Validation failed");
    }

    #[tokio::test]
    async fn rejected_error_maps_to_422() {
        let (status, _) =
            response_parts(ErrorStatus::from(Report::new(KernelError::Rejected))).await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
}
