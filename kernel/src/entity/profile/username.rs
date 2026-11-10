use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

use crate::KernelError;

/// Profile username: the name part of `acct:username@domain`.
///
/// Format `^[a-z0-9][a-z0-9._-]{0,63}$` (ASCII lowercase, 1-64 characters) plus a reserved-word
/// list. The value is never trimmed or case-folded implicitly. It is unique per Profile (also
/// across deleted Profiles), set at most once and immutable afterwards (ADR profile-actor-identity).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileUsername(String);

pub const PROFILE_USERNAME_MAX_LEN: usize = 64;

pub const RESERVED_PROFILE_USERNAMES: &[&str] = &[
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileUsernameError {
    Format,
    Reserved,
}

impl ProfileUsernameError {
    pub fn message(&self) -> &'static str {
        match self {
            ProfileUsernameError::Format => {
                "username must match ^[a-z0-9][a-z0-9._-]{0,63}$ (ASCII lowercase, 1-64 characters, no implicit trimming or case folding)"
            }
            ProfileUsernameError::Reserved => "username is reserved",
        }
    }
}

impl ProfileUsername {
    /// Validate `^[a-z0-9][a-z0-9._-]{0,63}$` plus the reserved-word list.
    pub fn check(username: &str) -> Result<(), ProfileUsernameError> {
        let mut chars = username.chars();
        let first_ok = chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        let rest_ok = chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
        if !first_ok || !rest_ok || username.len() > PROFILE_USERNAME_MAX_LEN {
            return Err(ProfileUsernameError::Format);
        }
        if RESERVED_PROFILE_USERNAMES.contains(&username) {
            return Err(ProfileUsernameError::Reserved);
        }
        Ok(())
    }

    /// Validating constructor; violations are `KernelError::Validation` (HTTP 400).
    pub fn parse(username: impl Into<String>) -> error_stack::Result<Self, KernelError> {
        let username = username.into();
        Self::check(&username).map_err(|e| {
            Report::new(KernelError::Validation).attach_printable(e.message().to_string())
        })?;
        Ok(Self(username))
    }

    pub fn validate(&self) -> error_stack::Result<(), KernelError> {
        Self::parse(self.0.clone()).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_usernames() {
        for name in ["a", "0", "alice", "a.b_c-d", "a".repeat(64).as_str()] {
            assert_eq!(ProfileUsername::check(name), Ok(()), "{name}");
            assert!(ProfileUsername::parse(name).is_ok());
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
                ProfileUsername::check(name),
                Err(ProfileUsernameError::Format),
                "{name:?}"
            );
        }
    }

    #[test]
    fn rejects_every_reserved_word() {
        for name in RESERVED_PROFILE_USERNAMES {
            assert_eq!(
                ProfileUsername::check(name),
                Err(ProfileUsernameError::Reserved),
                "{name}"
            );
        }
        assert_eq!(ProfileUsername::check("admin1"), Ok(()));
        assert_eq!(
            ProfileUsername::check("Admin"),
            Err(ProfileUsernameError::Format)
        );
    }

    #[test]
    fn parse_failure_is_a_validation_error() {
        let err = ProfileUsername::parse("Alice").unwrap_err();
        assert_eq!(err.current_context(), &KernelError::Validation);
    }

    #[test]
    fn serializes_as_a_plain_string() {
        let value = serde_json::to_value(ProfileUsername::new("alice")).unwrap();
        assert_eq!(value, serde_json::json!("alice"));
    }
}
