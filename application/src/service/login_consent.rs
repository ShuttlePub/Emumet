//! OAuth2 login / consent use cases: the branching, the scope validation and the shape of every
//! result of `/oauth2/login` and `/oauth2/consent`.
//!
//! Nothing here touches the database or knows an HTTP client: the OAuth2 provider and the identity
//! provider are reached only through the `kernel::interfaces::login_consent` ports, so the whole
//! unit runs against fakes and can move to another service as it is.

mod consent;
mod login;

#[cfg(test)]
mod ory_tests;
#[cfg(test)]
mod tests;

pub use self::consent::{ConsentDecision, ConsentOutcome, ConsentPrompt, ConsentUseCase};
pub use self::login::LoginUseCase;

use kernel::interfaces::login_consent::UpstreamError;
use std::fmt::{Display, Formatter};

/// How long the provider remembers an accepted login or consent.
pub const REMEMBER_FOR_SECS: i64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginConsentError {
    /// The OAuth2 provider failed or could not be reached (any status, any body problem).
    Upstream(UpstreamError),
    /// The consent grants a scope that the client never requested.
    ScopeNotRequested(String),
}

impl Display for LoginConsentError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginConsentError::Upstream(error) => write!(f, "OAuth2 provider error: {error}"),
            LoginConsentError::ScopeNotRequested(scope) => {
                write!(f, "scope was not requested: {scope}")
            }
        }
    }
}

impl std::error::Error for LoginConsentError {}
