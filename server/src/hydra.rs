use reqwest::Client;
use serde::{Deserialize, Serialize};
use url::Url;

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

    pub async fn get_login_request(&self, challenge: &str) -> Result<LoginRequest, reqwest::Error> {
        let url = self.build_url(
            "/admin/oauth2/auth/requests/login",
            "login_challenge",
            challenge,
        );
        self.http_client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<LoginRequest>()
            .await
    }

    pub async fn accept_login(
        &self,
        challenge: &str,
        body: &AcceptLoginRequest,
    ) -> Result<RedirectResponse, reqwest::Error> {
        let url = self.build_url(
            "/admin/oauth2/auth/requests/login/accept",
            "login_challenge",
            challenge,
        );
        self.http_client
            .put(url)
            .json(body)
            .send()
            .await?
            .error_for_status()?
            .json::<RedirectResponse>()
            .await
    }

    pub async fn reject_login(
        &self,
        challenge: &str,
        body: &RejectRequest,
    ) -> Result<RedirectResponse, reqwest::Error> {
        let url = self.build_url(
            "/admin/oauth2/auth/requests/login/reject",
            "login_challenge",
            challenge,
        );
        self.http_client
            .put(url)
            .json(body)
            .send()
            .await?
            .error_for_status()?
            .json::<RedirectResponse>()
            .await
    }

    pub async fn get_consent_request(
        &self,
        challenge: &str,
    ) -> Result<ConsentRequest, reqwest::Error> {
        let url = self.build_url(
            "/admin/oauth2/auth/requests/consent",
            "consent_challenge",
            challenge,
        );
        self.http_client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<ConsentRequest>()
            .await
    }

    pub async fn accept_consent(
        &self,
        challenge: &str,
        body: &AcceptConsentRequest,
    ) -> Result<RedirectResponse, reqwest::Error> {
        let url = self.build_url(
            "/admin/oauth2/auth/requests/consent/accept",
            "consent_challenge",
            challenge,
        );
        self.http_client
            .put(url)
            .json(body)
            .send()
            .await?
            .error_for_status()?
            .json::<RedirectResponse>()
            .await
    }

    pub async fn reject_consent(
        &self,
        challenge: &str,
        body: &RejectRequest,
    ) -> Result<RedirectResponse, reqwest::Error> {
        let url = self.build_url(
            "/admin/oauth2/auth/requests/consent/reject",
            "consent_challenge",
            challenge,
        );
        self.http_client
            .put(url)
            .json(body)
            .send()
            .await?
            .error_for_status()?
            .json::<RedirectResponse>()
            .await
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub challenge: String,
    pub skip: bool,
    pub subject: String,
    pub client: Option<OAuth2Client>,
    pub requested_scope: Vec<String>,
    pub requested_access_token_audience: Vec<String>,
    pub request_url: String,
}

#[derive(Debug, Deserialize)]
pub struct OAuth2Client {
    pub client_id: Option<String>,
    pub client_name: Option<String>,
    pub skip_consent: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct AcceptLoginRequest {
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remember: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remember_for: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ConsentRequest {
    pub challenge: String,
    pub skip: bool,
    pub subject: String,
    pub client: Option<OAuth2Client>,
    pub requested_scope: Vec<String>,
    pub requested_access_token_audience: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AcceptConsentRequest {
    pub grant_scope: Vec<String>,
    pub grant_access_token_audience: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remember: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remember_for: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<ConsentSession>,
}

#[derive(Debug, Serialize)]
pub struct ConsentSession {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id_token: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct RejectRequest {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RedirectResponse {
    pub redirect_to: String,
}
