//! Login / consent boundary: the OAuth2 provider's login and consent challenges and the identity
//! provider's session check, expressed as plain types and ports.
//!
//! This module is deliberately self-contained (no dependency on any other kernel entity or
//! repository) so that the whole login / consent unit can be carved out of Emumet as it is.
//!
//! * [`OAuth2FlowGateway`] is the port to the OAuth2 provider's admin API (Ory Hydra today).
//! * [`IdentitySessionVerifier`] is the port that verifies the browser's identity session (Ory Kratos
//!   today).
//!
//! The vendor wire formats stay inside the driver implementations; only the types below cross the
//! boundary.

use std::fmt::{Display, Formatter};
use std::future::Future;

/// Opaque login challenge issued by the OAuth2 provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginChallenge(String);

impl LoginChallenge {
    pub fn new(challenge: impl Into<String>) -> Self {
        Self(challenge.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque consent challenge issued by the OAuth2 provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentChallenge(String);

impl ConsentChallenge {
    pub fn new(challenge: impl Into<String>) -> Self {
        Self(challenge.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where the user agent must be sent next (the provider's continuation URL).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectTarget(String);

impl RedirectTarget {
    pub fn new(url: impl Into<String>) -> Self {
        Self(url.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

/// A login request as the provider describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginRequest {
    /// The provider already knows who the user is and asks only for the confirmation.
    pub skip: bool,
    /// The subject the provider already authenticated (meaningful only when `skip` is set).
    pub subject: String,
}

/// Claims Emumet attaches to the login so that the consent step can reuse them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginContext {
    pub email: Option<String>,
}

/// Accepts a login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptLogin {
    pub subject: String,
    /// Remember the authentication for `remember_for_secs`.
    pub remember: bool,
    pub remember_for_secs: i64,
    /// `None` attaches no context at all.
    pub context: Option<LoginContext>,
}

/// Refuses a login or a consent (OAuth2 `error` / `error_description`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub error: String,
    pub error_description: Option<String>,
}

/// The client the consent is requested for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsentClient {
    pub client_name: Option<String>,
    pub skip_consent: Option<bool>,
}

/// A consent request as the provider describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentRequest {
    pub skip: bool,
    pub client: Option<ConsentClient>,
    pub requested_scope: Vec<String>,
    pub requested_access_token_audience: Vec<String>,
    /// What was attached to the login this consent follows.
    pub context: LoginContext,
}

/// Accepts a consent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptConsent {
    pub grant_scope: Vec<String>,
    pub grant_access_token_audience: Vec<String>,
    /// Remember the grant for `remember_for_secs`.
    pub remember: bool,
    pub remember_for_secs: i64,
    /// `email` claim of the ID token; `None` adds no session claims.
    pub id_token_email: Option<String>,
}

/// An identity session that the identity provider confirmed as active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSession {
    /// Stable identity id; becomes the OAuth2 subject.
    pub identity_id: String,
    pub email: Option<String>,
}

/// How a call to the OAuth2 / identity provider failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamErrorKind {
    /// The provider could not be reached (connection failure, timeout).
    Unreachable,
    /// The provider answered with a non-success HTTP status.
    Status(u16),
    /// The provider answered, but not with the expected body.
    InvalidResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamError {
    kind: UpstreamErrorKind,
    detail: String,
}

impl UpstreamError {
    pub fn new(kind: UpstreamErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub fn kind(&self) -> UpstreamErrorKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl Display for UpstreamError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail)
    }
}

impl std::error::Error for UpstreamError {}

/// Port to the OAuth2 provider's admin API for the login and consent challenges.
pub trait OAuth2FlowGateway: Send + Sync + 'static {
    fn get_login_request(
        &self,
        challenge: &LoginChallenge,
    ) -> impl Future<Output = Result<LoginRequest, UpstreamError>> + Send;

    fn accept_login(
        &self,
        challenge: &LoginChallenge,
        accept: &AcceptLogin,
    ) -> impl Future<Output = Result<RedirectTarget, UpstreamError>> + Send;

    fn reject_login(
        &self,
        challenge: &LoginChallenge,
        rejection: &Rejection,
    ) -> impl Future<Output = Result<RedirectTarget, UpstreamError>> + Send;

    fn get_consent_request(
        &self,
        challenge: &ConsentChallenge,
    ) -> impl Future<Output = Result<ConsentRequest, UpstreamError>> + Send;

    fn accept_consent(
        &self,
        challenge: &ConsentChallenge,
        accept: &AcceptConsent,
    ) -> impl Future<Output = Result<RedirectTarget, UpstreamError>> + Send;

    fn reject_consent(
        &self,
        challenge: &ConsentChallenge,
        rejection: &Rejection,
    ) -> impl Future<Output = Result<RedirectTarget, UpstreamError>> + Send;
}

pub trait DependOnOAuth2FlowGateway: Send + Sync {
    type OAuth2FlowGateway: OAuth2FlowGateway;
    fn oauth2_flow_gateway(&self) -> &Self::OAuth2FlowGateway;
}

/// Port that verifies the identity session of the browser that arrives at the login endpoint.
pub trait IdentitySessionVerifier: Send + Sync + 'static {
    /// `session_cookie` is the single `name=value` identity session cookie, already separated from
    /// the other cookies of the request; nothing else is forwarded to the provider.
    ///
    /// `Ok(None)` means the provider answered that the session is unknown, expired or inactive.
    /// A provider that cannot be reached or answers with an error is an `Err`.
    fn verify_session(
        &self,
        session_cookie: &str,
    ) -> impl Future<Output = Result<Option<VerifiedSession>, UpstreamError>> + Send;
}

pub trait DependOnIdentitySessionVerifier: Send + Sync {
    type IdentitySessionVerifier: IdentitySessionVerifier;
    fn identity_session_verifier(&self) -> &Self::IdentitySessionVerifier;
}
