//! Iceshrimp REST API client for ActivityPub federation E2E tests.
//!
//! Iceshrimp exposes a Misskey-compatible API. All endpoints are POST with JSON body.
//! Authentication uses the `i` field in the request body (token-based auth).

use reqwest::Client;

use super::account_helper::e2e_http_client;

/// Client for interacting with an Iceshrimp instance via its REST API.
pub struct IceshrimpClient {
    pub base_url: String,
    client: Client,
}

impl IceshrimpClient {
    /// Create a new client that connects to the given base URL.
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: e2e_http_client(),
        }
    }

    /// POST /api/signup — create a new user account.
    ///
    /// Returns the full JSON response, which includes the token in `["i"]`.
    ///
    /// Registration must be open on the Iceshrimp instance (default for dev setups).
    pub async fn signup(
        &self,
        username: &str,
        password: &str,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let body = serde_json::json!({
            "username": username,
            "password": password,
        });
        self.post_json("/api/signup", &body).await
    }

    /// POST /api/signin — log in and return the session token.
    ///
    /// The returned token can be used as the `i` field in subsequent requests.
    pub async fn login(
        &self,
        username: &str,
        password: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let body = serde_json::json!({
            "username": username,
            "password": password,
        });
        let resp = self.post_json("/api/signin", &body).await?;
        let token = resp["i"]
            .as_str()
            .ok_or_else(|| "signin response missing 'i' token".to_string())?;
        Ok(token.to_string())
    }

    /// POST /api/users/show — get user info by internal ID.
    pub async fn show_user(
        &self,
        user_id: &str,
        token: &str,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let body = serde_json::json!({
            "userId": user_id,
            "i": token,
        });
        self.post_json("/api/users/show", &body).await
    }

    /// POST /api/ap/show — resolve a remote ActivityPub actor by its URI.
    ///
    /// The `uri` should be a `acct:user@host` WebFinger identifier.
    /// Returns the resolved actor info from the remote instance's perspective.
    pub async fn resolve_remote_user(
        &self,
        uri: &str,
        token: &str,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let body = serde_json::json!({
            "uri": uri,
            "i": token,
        });
        self.post_json("/api/ap/show", &body).await
    }

    /// POST /api/i — get the authenticated user's full profile.
    ///
    /// This returns the full detail view including `publicKey` (if the key exists).
    pub async fn my_profile(
        &self,
        token: &str,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let body = serde_json::json!({
            "i": token,
        });
        self.post_json("/api/i", &body).await
    }

    /// POST /api/following/create — send a Follow activity to the given user.
    pub async fn follow_user(
        &self,
        user_id: &str,
        token: &str,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let body = serde_json::json!({
            "userId": user_id,
            "i": token,
        });
        self.post_json("/api/following/create", &body).await
    }

    /// POST /api/users/followers — get the list of followers for a user.
    pub async fn get_followers(
        &self,
        user_id: &str,
        token: &str,
    ) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
        let body = serde_json::json!({
            "userId": user_id,
            "i": token,
        });
        self.post_json_array("/api/users/followers", &body).await
    }

    /// POST /api/users/following — get the list of users this user follows.
    pub async fn get_following(
        &self,
        user_id: &str,
        token: &str,
    ) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
        let body = serde_json::json!({
            "userId": user_id,
            "i": token,
        });
        self.post_json_array("/api/users/following", &body).await
    }

    /// POST /api/notes/global-timeline — fetch public/global timeline notes.
    ///
    /// Returns the most recent notes visible on the global timeline
    /// (up to `limit` items). Remote notes are included if the instance
    /// knows about them.
    pub async fn get_global_timeline(
        &self,
        token: &str,
        limit: u32,
    ) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
        let body = serde_json::json!({
            "i": token,
            "limit": limit,
            "withRenotes": false,
        });
        self.post_json_array("/api/notes/global-timeline", &body)
            .await
    }

    /// Check whether the global timeline contains a note with the given
    /// ActivityPub `uri`.
    pub async fn global_timeline_contains_uri(
        &self,
        token: &str,
        uri: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let notes = self.get_global_timeline(token, 20).await?;
        Ok(notes
            .iter()
            .any(|note| note.get("uri").and_then(|v| v.as_str()) == Some(uri)))
    }

    // ── private helpers ──────────────────────────────────────────

    /// Send a POST request and deserialize the body as a JSON value.
    async fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.client.post(&url).json(body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Iceshrimp {path} failed: status={status}, body={text}").into());
        }
        Ok(resp.json().await?)
    }

    /// Send a POST request and deserialize the body as a JSON array.
    async fn post_json_array(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.client.post(&url).json(body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Iceshrimp {path} failed: status={status}, body={text}").into());
        }
        Ok(resp.json().await?)
    }
}
