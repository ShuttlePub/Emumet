use axum::http::StatusCode;

use crate::error::ErrorStatus;

pub const USERNAME_MAX_LEN: usize = 64;

pub const RESERVED_USERNAMES: &[&str] = &[
    "admin",
    "administrator",
    "root",
    "system",
    "support",
    "security",
    "moderator",
    "api",
    "ap",
    "auth",
    "oauth",
    "oauth2",
    "login",
    "logout",
    "register",
    "settings",
    "well-known",
];

#[derive(Debug, PartialEq, Eq)]
pub enum UsernameError {
    Format,
    Reserved,
}

impl UsernameError {
    pub fn message(&self) -> &'static str {
        match self {
            UsernameError::Format => {
                "username must match ^[a-z0-9][a-z0-9._-]{0,63}$ (ASCII lowercase, 1-64 characters, no implicit trimming or case folding)"
            }
            UsernameError::Reserved => "username is reserved",
        }
    }
}

/// Validate `^[a-z0-9][a-z0-9._-]{0,63}$` plus the reserved-word list.
///
/// The input is never trimmed or lowercased: `" alice"` and `"Alice"` are format errors.
pub fn validate_profile_username(username: &str) -> Result<(), UsernameError> {
    let mut chars = username.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let rest_ok =
        chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if !first_ok || !rest_ok || username.len() > USERNAME_MAX_LEN {
        return Err(UsernameError::Format);
    }
    if RESERVED_USERNAMES.contains(&username) {
        return Err(UsernameError::Reserved);
    }
    Ok(())
}

/// Gate for optional `username` inputs while username storage does not exist yet (E8..P1).
///
/// Absent: `Ok(())` (legacy payloads keep working). Present and malformed or reserved: 400.
/// Present and valid: 501, because silently dropping the value is not allowed. Callers MUST
/// invoke this before the creation API's own side effects (Account/Profile/event creation, and
/// the handler's auth-account resolution).
pub fn reject_unstored_username(username: Option<&str>) -> Result<(), ErrorStatus> {
    let Some(username) = username else {
        return Ok(());
    };
    validate_profile_username(username)
        .map_err(|e| ErrorStatus::from((StatusCode::BAD_REQUEST, e.message().to_string())))?;
    Err(ErrorStatus::from((
        StatusCode::NOT_IMPLEMENTED,
        "profile username storage is not implemented yet; omit the username field".to_string(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_usernames() {
        for name in ["a", "0", "alice", "a.b_c-d", "a".repeat(64).as_str()] {
            assert_eq!(validate_profile_username(name), Ok(()), "{name}");
        }
    }

    #[test]
    fn rejects_malformed_usernames_without_normalizing() {
        for name in [
            "",
            " alice",
            "alice ",
            "Alice",
            "alice@host",
            "-alice",
            ".alice",
            "_alice",
            "ali/ce",
            "ａlice",
            "alicé",
            "a\n",
            "a".repeat(65).as_str(),
        ] {
            assert_eq!(
                validate_profile_username(name),
                Err(UsernameError::Format),
                "{name:?}"
            );
        }
    }

    #[test]
    fn rejects_every_reserved_word() {
        for name in RESERVED_USERNAMES {
            assert_eq!(
                validate_profile_username(name),
                Err(UsernameError::Reserved),
                "{name}"
            );
        }
        assert_eq!(validate_profile_username("admin1"), Ok(()));
        assert_eq!(
            validate_profile_username("Admin"),
            Err(UsernameError::Format)
        );
    }

    #[test]
    fn gate_passes_absent_and_blocks_present() {
        assert!(reject_unstored_username(None).is_ok());
        let err = reject_unstored_username(Some("alice")).unwrap_err();
        assert!(matches!(
            err,
            ErrorStatus::StatusCodeWithMessage(StatusCode::NOT_IMPLEMENTED, _)
        ));
        let err = reject_unstored_username(Some("Alice")).unwrap_err();
        assert!(matches!(
            err,
            ErrorStatus::StatusCodeWithMessage(StatusCode::BAD_REQUEST, _)
        ));
    }
}
