//! Ory Hydra admin API client: the `OAuth2FlowGateway` implementation for the login and consent
//! challenges. The Hydra wire format stays in this file.

use kernel::interfaces::login_consent::{
    AcceptConsent, AcceptLogin, ConsentChallenge, ConsentClient, ConsentRequest, LoginChallenge,
    LoginContext, LoginRequest, OAuth2FlowGateway, RedirectTarget, Rejection, UpstreamError,
    UpstreamErrorKind,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone)]
pub struct HydraAdminClient {
    admin_url: String,
    http_client: Client,
}

impl HydraAdminClient {
    /// Create a new HydraAdminClient. Panics if `admin_url` is not a valid URL.
    pub fn new(admin_url: String) -> Self {
        let admin_url = admin_url.trim_end_matches('/').to_string();
        // Validate URL at construction time to fail fast.
        Url::parse(&admin_url)
            .unwrap_or_else(|e| panic!("HYDRA_ADMIN_URL is not a valid URL ({admin_url}): {e}"));
        Self {
            admin_url,
            http_client: Client::new(),
        }
    }

    /// Build a URL with a properly encoded challenge query parameter.
    /// The base URL was validated in `new()`, so `parse_with_params` will not fail.
    fn build_url(&self, path: &str, param_name: &str, challenge: &str) -> Url {
        let base = format!("{}{}", self.admin_url, path);
        Url::parse_with_params(&base, &[(param_name, challenge)])
            .expect("base URL was validated at construction time")
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        param_name: &str,
        challenge: &str,
    ) -> Result<T, UpstreamError> {
        let url = self.build_url(path, param_name, challenge);
        self.http_client
            .get(url)
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(upstream_error)?
            .json::<T>()
            .await
            .map_err(upstream_error)
    }

    async fn put<B: Serialize>(
        &self,
        path: &str,
        param_name: &str,
        challenge: &str,
        body: &B,
    ) -> Result<RedirectTarget, UpstreamError> {
        let url = self.build_url(path, param_name, challenge);
        let redirect = self
            .http_client
            .put(url)
            .json(body)
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(upstream_error)?
            .json::<RedirectResponse>()
            .await
            .map_err(upstream_error)?;
        Ok(RedirectTarget::new(redirect.redirect_to))
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
    UpstreamError::new(kind, format!("Hydra admin API request failed: {error}"))
}

impl OAuth2FlowGateway for HydraAdminClient {
    async fn get_login_request(
        &self,
        challenge: &LoginChallenge,
    ) -> Result<LoginRequest, UpstreamError> {
        let request = self
            .get::<WireLoginRequest>(
                "/admin/oauth2/auth/requests/login",
                "login_challenge",
                challenge.as_str(),
            )
            .await?;
        Ok(LoginRequest {
            skip: request.skip,
            subject: request.subject,
        })
    }

    async fn accept_login(
        &self,
        challenge: &LoginChallenge,
        accept: &AcceptLogin,
    ) -> Result<RedirectTarget, UpstreamError> {
        let body = WireAcceptLogin {
            subject: &accept.subject,
            remember: Some(accept.remember),
            remember_for: Some(accept.remember_for_secs),
            context: accept.context.as_ref().and_then(login_context_to_json),
        };
        self.put(
            "/admin/oauth2/auth/requests/login/accept",
            "login_challenge",
            challenge.as_str(),
            &body,
        )
        .await
    }

    async fn reject_login(
        &self,
        challenge: &LoginChallenge,
        rejection: &Rejection,
    ) -> Result<RedirectTarget, UpstreamError> {
        self.put(
            "/admin/oauth2/auth/requests/login/reject",
            "login_challenge",
            challenge.as_str(),
            &WireReject::from(rejection),
        )
        .await
    }

    async fn get_consent_request(
        &self,
        challenge: &ConsentChallenge,
    ) -> Result<ConsentRequest, UpstreamError> {
        let request = self
            .get::<WireConsentRequest>(
                "/admin/oauth2/auth/requests/consent",
                "consent_challenge",
                challenge.as_str(),
            )
            .await?;
        Ok(ConsentRequest {
            skip: request.skip,
            client: request.client.map(|client| ConsentClient {
                client_name: client.client_name,
                skip_consent: client.skip_consent,
            }),
            requested_scope: request.requested_scope,
            requested_access_token_audience: request.requested_access_token_audience,
            context: LoginContext {
                email: request
                    .context
                    .get("email")
                    .and_then(|value| value.as_str())
                    .map(String::from),
            },
        })
    }

    async fn accept_consent(
        &self,
        challenge: &ConsentChallenge,
        accept: &AcceptConsent,
    ) -> Result<RedirectTarget, UpstreamError> {
        let body = WireAcceptConsent {
            grant_scope: &accept.grant_scope,
            grant_access_token_audience: &accept.grant_access_token_audience,
            remember: Some(accept.remember),
            remember_for: Some(accept.remember_for_secs),
            session: accept
                .id_token_email
                .as_ref()
                .map(|email| WireConsentSession {
                    id_token: serde_json::json!({ "email": email }),
                }),
        };
        self.put(
            "/admin/oauth2/auth/requests/consent/accept",
            "consent_challenge",
            challenge.as_str(),
            &body,
        )
        .await
    }

    async fn reject_consent(
        &self,
        challenge: &ConsentChallenge,
        rejection: &Rejection,
    ) -> Result<RedirectTarget, UpstreamError> {
        self.put(
            "/admin/oauth2/auth/requests/consent/reject",
            "consent_challenge",
            challenge.as_str(),
            &WireReject::from(rejection),
        )
        .await
    }
}

/// A context without an email carries nothing, so it is not sent at all.
fn login_context_to_json(context: &LoginContext) -> Option<serde_json::Value> {
    context
        .email
        .as_ref()
        .map(|email| serde_json::json!({ "email": email }))
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // every field is required in the response, whether or not it is read
struct WireLoginRequest {
    challenge: String,
    skip: bool,
    subject: String,
    client: Option<WireClient>,
    requested_scope: Vec<String>,
    requested_access_token_audience: Vec<String>,
    request_url: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // every field is required in the response, whether or not it is read
struct WireConsentRequest {
    challenge: String,
    skip: bool,
    subject: String,
    client: Option<WireClient>,
    requested_scope: Vec<String>,
    requested_access_token_audience: Vec<String>,
    #[serde(default)]
    context: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WireClient {
    client_id: Option<String>,
    client_name: Option<String>,
    skip_consent: Option<bool>,
}

#[derive(Debug, Serialize)]
struct WireAcceptLogin<'a> {
    subject: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    remember: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remember_for: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct WireAcceptConsent<'a> {
    grant_scope: &'a [String],
    grant_access_token_audience: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    remember: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remember_for: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<WireConsentSession>,
}

#[derive(Debug, Serialize)]
struct WireConsentSession {
    id_token: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct WireReject<'a> {
    error: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_description: Option<&'a str>,
}

impl<'a> From<&'a Rejection> for WireReject<'a> {
    fn from(rejection: &'a Rejection) -> Self {
        Self {
            error: &rejection.error,
            error_description: rejection.error_description.as_deref(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RedirectResponse {
    redirect_to: String,
}

#[cfg(test)]
#[path = "hydra/tests.rs"]
mod tests;
