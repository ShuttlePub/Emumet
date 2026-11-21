//! Ory Kratos public API client: the `IdentitySessionVerifier` implementation (`/sessions/whoami`).

use kernel::interfaces::login_consent::{
    IdentitySessionVerifier, UpstreamError, UpstreamErrorKind, VerifiedSession,
};
use reqwest::Client;
use serde::Deserialize;
use url::Url;

#[derive(Clone)]
pub struct KratosSessionClient {
    public_url: String,
    http_client: Client,
}

impl KratosSessionClient {
    /// Create a new client. Panics if `public_url` is not a valid URL.
    pub fn new(public_url: String) -> Self {
        let public_url = public_url.trim_end_matches('/').to_string();
        Url::parse(&public_url)
            .unwrap_or_else(|e| panic!("KRATOS_PUBLIC_URL is not a valid URL ({public_url}): {e}"));
        Self {
            public_url,
            http_client: Client::new(),
        }
    }
}

fn upstream_error(error: reqwest::Error) -> UpstreamError {
    let kind = if let Some(status) = error.status() {
        UpstreamErrorKind::Status(status.as_u16())
    } else if error.is_decode() {
        UpstreamErrorKind::InvalidResponse
    } else {
        UpstreamErrorKind::Unreachable
    };
    UpstreamError::new(kind, format!("Kratos whoami request failed: {error}"))
}

impl IdentitySessionVerifier for KratosSessionClient {
    /// Calls `/sessions/whoami` with only `session_cookie` as the `Cookie` header. No session
    /// (401) and an inactive session are `Ok(None)`.
    async fn verify_session(
        &self,
        session_cookie: &str,
    ) -> Result<Option<VerifiedSession>, UpstreamError> {
        let url = format!("{}/sessions/whoami", self.public_url);
        tracing::debug!("Calling Kratos whoami: url={url}");

        let response = self
            .http_client
            .get(&url)
            .header("cookie", session_cookie)
            .send()
            .await
            .map_err(upstream_error)?;

        let status = response.status();
        tracing::debug!("Kratos whoami response: status={status}");

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Ok(None);
        }

        let session = response
            .error_for_status()
            .map_err(upstream_error)?
            .json::<WireSession>()
            .await
            .map_err(upstream_error)?;
        tracing::debug!(
            "Kratos whoami session: id={}, active={}",
            session.id,
            session.active
        );
        if !session.active {
            tracing::debug!("Kratos whoami: session is not active, treating as unauthenticated");
            return Ok(None);
        }
        Ok(Some(VerifiedSession {
            identity_id: session.identity.id,
            email: session
                .identity
                .traits
                .get("email")
                .and_then(|value| value.as_str())
                .map(String::from),
        }))
    }
}

#[derive(Debug, Deserialize)]
struct WireSession {
    id: String,
    #[serde(default)]
    active: bool,
    identity: WireIdentity,
}

#[derive(Debug, Deserialize)]
struct WireIdentity {
    id: String,
    #[serde(default)]
    traits: serde_json::Value,
}

#[cfg(test)]
#[path = "kratos/tests.rs"]
mod tests;
