//! Mastodon REST API client for ActivityPub federation E2E tests.
//!
//! Mastodon exposes a RESTful API. Authentication uses Bearer tokens
//! (OAuth2).  Account creation requires an OAuth app token with the
//! `write:accounts` scope.

use reqwest::Client;

use super::account_helper::e2e_http_client;

/// Client for interacting with a Mastodon instance via its REST API.
pub struct MastodonClient {
    pub base_url: String,
    client: Client,
}

/// Response from POST /api/v1/apps
pub struct MastodonApp {
    pub client_id: String,
    pub client_secret: String,
}

/// Response from POST /api/v1/accounts (the Token entity)
pub struct MastodonAccountCreation {
    pub access_token: String,
}

impl MastodonClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: e2e_http_client(),
        }
    }

    /// POST /api/v1/apps — register an OAuth application.
    ///
    /// Returns the client_id and client_secret needed for token acquisition.
    pub async fn create_app(
        &self,
        client_name: &str,
        scopes: &str,
    ) -> Result<MastodonApp, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/api/v1/apps", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "client_name": client_name,
                "redirect_uris": "urn:ietf:wg:oauth:2.0:oob",
                "scopes": scopes,
            }))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("POST /api/v1/apps failed: status={status}, body={text}").into());
        }
        let body: serde_json::Value = resp.json().await?;
        let client_id = body["client_id"]
            .as_str()
            .ok_or_else(|| "app response missing client_id".to_string())?
            .to_string();
        let client_secret = body["client_secret"]
            .as_str()
            .ok_or_else(|| "app response missing client_secret".to_string())?
            .to_string();
        Ok(MastodonApp {
            client_id,
            client_secret,
        })
    }

    /// POST /oauth/token with grant_type=client_credentials.
    ///
    /// Returns an app-level access token.
    pub async fn get_client_token(
        &self,
        client_id: &str,
        client_secret: &str,
        scope: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/oauth/token", self.base_url);
        let resp = self
            .client
            .post(&url)
            .form(&[
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("grant_type", "client_credentials"),
                ("scope", scope),
                ("redirect_uri", "urn:ietf:wg:oauth:2.0:oob"),
            ])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "POST /oauth/token (client_credentials) failed: status={status}, body={text}"
            )
            .into());
        }
        let body: serde_json::Value = resp.json().await?;
        let token = body["access_token"]
            .as_str()
            .ok_or_else(|| "token response missing access_token".to_string())?
            .to_string();
        Ok(token)
    }

    /// POST /api/v1/accounts — create a new user account.
    ///
    /// Requires an app-level token with the `write:accounts` scope.
    /// Returns the user access token embedded in the response.
    pub async fn create_account(
        &self,
        app_token: &str,
        username: &str,
        password: &str,
        email: &str,
    ) -> Result<MastodonAccountCreation, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/api/v1/accounts", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {app_token}"))
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "username": username,
                "password": password,
                "email": email,
                "agreement": true,
                "locale": "en",
            }))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(
                format!("POST /api/v1/accounts failed: status={status}, body={text}").into(),
            );
        }
        let body: serde_json::Value = resp.json().await?;
        let access_token = body["access_token"]
            .as_str()
            .ok_or_else(|| "account creation response missing access_token".to_string())?
            .to_string();
        Ok(MastodonAccountCreation { access_token })
    }

    /// GET /api/v1/accounts/verify_credentials — get the authenticated user's
    /// own account info.
    pub async fn verify_credentials(
        &self,
        token: &str,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/api/v1/accounts/verify_credentials", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "GET /api/v1/accounts/verify_credentials failed: status={status}, body={text}"
            )
            .into());
        }
        Ok(resp.json().await?)
    }

    /// GET /api/v2/search — search and optionally resolve a remote ActivityPub
    /// actor by its URI.
    ///
    /// With `resolve=true`, Mastodon will fetch the remote actor via
    /// ActivityPub and cache it locally.
    ///
    /// Returns the first matching account JSON object.
    pub async fn search_remote_account(
        &self,
        uri: &str,
        token: &str,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/api/v2/search", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .query(&[
                ("q", uri),
                ("type", "accounts"),
                ("resolve", "true"),
                ("limit", "1"),
            ])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("GET /api/v2/search failed: status={status}, body={text}").into());
        }
        let body: serde_json::Value = resp.json().await?;
        let accounts = body["accounts"]
            .as_array()
            .ok_or_else(|| "search response missing accounts array".to_string())?;
        accounts
            .first()
            .cloned()
            .ok_or_else(|| format!("search returned no accounts for uri: {uri}").into())
    }

    /// POST /api/v1/accounts/{account_id}/follow — send a Follow activity.
    pub async fn follow_account(
        &self,
        account_id: &str,
        token: &str,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/api/v1/accounts/{account_id}/follow", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "POST /api/v1/accounts/{account_id}/follow failed: status={status}, body={text}"
            )
            .into());
        }
        Ok(resp.json().await?)
    }

    /// GET /api/v1/accounts/{account_id}/followers — get the list of
    /// followers.
    pub async fn get_followers(
        &self,
        account_id: &str,
        token: &str,
    ) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/api/v1/accounts/{account_id}/followers", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "GET /api/v1/accounts/{account_id}/followers failed: status={status}, body={text}"
            )
            .into());
        }
        Ok(resp.json().await?)
    }

    /// GET /api/v1/accounts/{account_id}/following — get the list of
    /// accounts this user follows.
    pub async fn get_following(
        &self,
        account_id: &str,
        token: &str,
    ) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/api/v1/accounts/{account_id}/following", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "GET /api/v1/accounts/{account_id}/following failed: status={status}, body={text}"
            )
            .into());
        }
        Ok(resp.json().await?)
    }

    /// GET /api/v1/timelines/public — fetch the public (local) timeline.
    ///
    /// Returns the most recent public posts visible on the instance.
    pub async fn get_public_timeline(
        &self,
        token: &str,
        limit: u32,
    ) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/api/v1/timelines/public", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .query(&[("local", "false"), ("limit", &limit.to_string())])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "GET /api/v1/timelines/public failed: status={status}, body={text}"
            )
            .into());
        }
        Ok(resp.json().await?)
    }

    /// Check whether the public (local) timeline contains a status with the
    /// given ActivityPub `uri`.
    pub async fn public_timeline_contains_uri(
        &self,
        token: &str,
        uri: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let statuses = self.get_public_timeline(token, 40).await?;
        Ok(statuses
            .iter()
            .any(|s| s.get("uri").and_then(|v| v.as_str()) == Some(uri)))
    }

    /// GET /api/v1/accounts/{account_id}/statuses — get the account's posts.
    ///
    /// Useful for verifying delivery of a Create/Note to the timeline.
    pub async fn get_account_statuses(
        &self,
        account_id: &str,
        token: &str,
    ) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/api/v1/accounts/{account_id}/statuses", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .query(&[("limit", "40")])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "GET /api/v1/accounts/{account_id}/statuses failed: status={status}, body={text}"
            )
            .into());
        }
        Ok(resp.json().await?)
    }
}
