use axum::http::StatusCode;
use kernel::prelude::entity::ProfileUsername;

use crate::error::ErrorStatus;

/// Pre-flight validation of a `username` input (400 on format / reserved-word violations).
/// Absent values pass; use [`require_username`] where the username is mandatory. Callers MUST invoke
/// this before the API's own side effects (Account/Profile/event creation, and the handler's
/// auth-account resolution) so that a malformed value never creates anything. The value itself is
/// stored by the use case.
pub fn validate_optional_username(username: Option<&str>) -> Result<(), ErrorStatus> {
    let Some(username) = username else {
        return Ok(());
    };
    ProfileUsername::check(username)
        .map_err(|e| ErrorStatus::from((StatusCode::BAD_REQUEST, e.message().to_string())))
}

/// Username is required on the creation APIs (P4): a missing value is a 400 validation error, as
/// are the format / reserved-word violations, all before any side effect.
pub fn require_username(username: Option<&str>) -> Result<String, ErrorStatus> {
    let Some(username) = username else {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "username is required".to_string(),
        )));
    };
    validate_optional_username(Some(username))?;
    Ok(username.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_gate_rejects_absent_values_with_400_and_returns_valid_ones() {
        let err = require_username(None).unwrap_err();
        assert!(matches!(
            err,
            ErrorStatus::StatusCodeWithMessage(StatusCode::BAD_REQUEST, _)
        ));
        assert_eq!(require_username(Some("a.b_c-d")).unwrap(), "a.b_c-d");
        assert!(require_username(Some("Alice")).is_err());
    }

    #[test]
    fn gate_passes_absent_and_valid_values() {
        assert!(validate_optional_username(None).is_ok());
        assert!(validate_optional_username(Some("alice")).is_ok());
    }

    #[test]
    fn gate_rejects_malformed_and_reserved_values_with_400() {
        for value in ["Alice", "", " alice", "admin", "well-known"] {
            let err = validate_optional_username(Some(value)).unwrap_err();
            assert!(
                matches!(
                    err,
                    ErrorStatus::StatusCodeWithMessage(StatusCode::BAD_REQUEST, _)
                ),
                "{value:?}"
            );
        }
    }
}
