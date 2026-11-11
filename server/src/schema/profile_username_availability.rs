use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Query of the username availability check.
#[derive(Debug, Deserialize)]
pub(crate) struct ProfileUsernameAvailabilityQuery {
    pub(crate) username: String,
}

/// Why a username cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileUsernameUnavailableReason {
    /// Does not match `^[a-z0-9][a-z0-9._-]{0,63}$` (ASCII lowercase letters, digits, `.`, `_`, `-`;
    /// 1-64 characters; the first character is a letter or digit). The value is never trimmed or
    /// case-folded, so `Alice` and ` alice` are malformed.
    InvalidFormat,
    /// One of the reserved words (e.g. `admin`, `api`, `well-known`).
    Reserved,
    /// Already in use, or used by a Profile that has since been deleted (a username is never
    /// reused). The two cases are deliberately not distinguished.
    Taken,
}

/// Result of a username availability check. Advisory only: the create / set username APIs make
/// the final decision.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileUsernameAvailabilityResponse {
    /// The checked value, exactly as received.
    pub username: String,
    /// `true` when the username could be assigned right now.
    pub available: bool,
    /// Present only when `available` is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<ProfileUsernameUnavailableReason>,
}
