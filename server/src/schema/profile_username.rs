use axum::http::StatusCode;
use kernel::prelude::entity::ProfileUsername;

use crate::error::ErrorStatus;

/// Pre-flight validation of an optional `username` input (400 on format / reserved-word
/// violations). Absent values pass. Callers MUST invoke this before the creation API's own side
/// effects (Account/Profile/event creation, and the handler's auth-account resolution) so that a
/// malformed value never creates anything. The value itself is stored by the use case.
pub fn validate_optional_username(username: Option<&str>) -> Result<(), ErrorStatus> {
    let Some(username) = username else {
        return Ok(());
    };
    ProfileUsername::check(username)
        .map_err(|e| ErrorStatus::from((StatusCode::BAD_REQUEST, e.message().to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

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
