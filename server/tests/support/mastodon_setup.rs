//! Shared setup helpers for Mastodon ActivityPub E2E tests.
//!
//! Follows the same pattern as `iceshrimp_setup.rs` but for Mastodon's OAuth2
//! REST API.  The OAuth flow is:
//!   1. Create an app (POST /api/v1/apps) -> client_id, client_secret
//!   2. Get client token (POST /oauth/token) -> app-level access_token
//!   3. Create account (POST /api/v1/accounts) -> user access_token
//!   4. Verify credentials (GET /api/v1/accounts/verify_credentials) -> account info
//!
//! If account creation succeeds but `verify_credentials` fails, the helper
//! falls back to `docker exec` to confirm/approve the account via `tootctl`.

use super::account_helper::fetch_collection;
use super::config::ap_e2e_config;

pub struct MastodonUser {
    pub username: String,
    pub token: String,
    pub account_id: String,
    pub actor_url: String,
    pub inbox_url: String,
}

pub struct MastodonFixture {
    pub client: super::mastodon::MastodonClient,
    pub user: MastodonUser,
}

// -- Guards --------------------------------------------------------

pub fn require_ap_e2e_external_server(test_name: &str) {
    if std::env::var("EMUMET_E2E_EXTERNAL_SERVER").as_deref() != Ok("1") {
        panic!(
            "{test_name} requires EMUMET_E2E_EXTERNAL_SERVER=1\n\
             This test needs the compose environment (compose.yml + compose.ap-e2e.yml)\n\
             and Emumet running in test-mode."
        );
    }
}

// -- docker exec tootctl helpers ----------------------------------

async fn confirm_mastodon_account(username: &str) {
    let container = "emumet-ap-e2e-mastodon-web";
    // Mastodon v4.6.2: tootctl accounts modify --confirm --approve
    let output = tokio::process::Command::new("docker")
        .args([
            "exec",
            container,
            "bin/tootctl",
            "accounts",
            "modify",
            username,
            "--confirm",
            "--approve",
        ])
        .output()
        .await
        .unwrap_or_else(|e| {
            panic!("docker exec tootctl accounts modify --confirm --approve failed: {e}");
        });
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "tootctl accounts modify {username} --confirm --approve failed (exit={}): {stderr}",
            output.status
        );
    }
}

// --- Setup helpers -----------------------------------------------

pub async fn signup_mastodon_user() -> MastodonFixture {
    let mastodon_base_url = std::env::var("MASTODON_BASE_URL")
        .expect("MASTODON_BASE_URL must be set for Mastodon E2E test");
    let client = super::mastodon::MastodonClient::new(&mastodon_base_url);

    let unique_suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let username = format!("e2e_{unique_suffix}");
    let email = format!("e2e_{unique_suffix}@example.com");

    // Use explicit resource-level scopes for Mastodon v4.6+;
    // the broad-group scopes (read, write, follow) also work but explicit
    // scopes make the intent clear and avoid ambiguity.
    const SCOPES: &str = "write:accounts write:follows read:accounts read:search read:statuses";

    let app = client
        .create_app("EmumetE2E", SCOPES)
        .await
        .expect("Failed to create Mastodon OAuth app");

    let app_token = client
        .get_client_token(&app.client_id, &app.client_secret, SCOPES)
        .await
        .expect("Failed to get Mastodon client token");

    let creation = client
        .create_account(&app_token, &username, "test-pass", &email)
        .await
        .expect("Failed to create Mastodon account");

    // Always confirm/approve the account to ensure it can federate.
    // tootctl operations are idempotent (harmless if already done).
    confirm_mastodon_account(&username).await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let verify = client
        .verify_credentials(&creation.access_token)
        .await
        .expect("Failed to verify Mastodon credentials even after tootctl confirm/approve");

    let account_id = verify["id"]
        .as_str()
        .expect("verify_credentials response missing id")
        .to_string();

    // Mastodon v4.x ActivityPub paths use /ap/users/{numeric_id} format.
    // The `uri` field from the API returns the correct ActivityPub actor ID,
    // which includes the `/ap/` prefix.  Federation traffic uses no port
    // (LOCAL_DOMAIN default port 443) for cache keys, and port 18443 for
    // outbound delivery (rootless Docker can't bind privileged port 443).
    let actor_uri = verify["uri"]
        .as_str()
        .expect("verify_credentials response missing uri");
    // actor_url uses LOCAL_DOMAIN (no port) — matches the cache key in
    // `inject_mastodon_actor_into_emumet_cache` so that `resolve_remote_actor`
    // finds the cached entry without making an HTTP fetch.
    let actor_url = actor_uri.to_string();
    // inbox_url uses the federation port 18443 so that outbound delivery
    // from the host reaches nginx (port 443 is unavailable rootless Docker).
    let inbox_url = actor_uri.replace(
        "//mastodon.127.0.0.1.nip.io/",
        "//mastodon.127.0.0.1.nip.io:18443/",
    ) + "/inbox";

    MastodonFixture {
        client,
        user: MastodonUser {
            username,
            token: creation.access_token,
            account_id,
            actor_url,
            inbox_url,
        },
    }
}

pub async fn setup_mastodon_remote_actor() -> (super::config::ApE2eConfig, MastodonFixture) {
    let cfg = ap_e2e_config();
    let mut fixture = signup_mastodon_user().await;
    inject_mastodon_actor_into_emumet_cache(&cfg, &mut fixture.user).await;
    (cfg, fixture)
}

/// Fetch the Mastodon actor's public key PEM from the database via
/// `docker exec psql`, then inject it and the actor data into Emumet's
/// global test caches (actor key + remote actor).
///
/// This is needed because Emumet runs on the host where port 443 is
/// unavailable (rootless Docker).  The actor URL from Mastodon's
/// activities defaults to port 443 (no port in LOCAL_DOMAIN), which
/// would cause connection failure.  By pre-populating the cache,
/// `resolve_remote_actor` and the HTTP Signature verifier return
/// cached data without making a remote fetch.
///
/// We use `docker exec psql` because Mastodon's ActivityPub actor
/// endpoint returns 404 for requests routed through the compose nginx
/// (the compose's port mapping 18443:443 means nginx receives requests
/// on container port 443 where `listen 18443` is not active, and
/// the Host header mismatch causes Mastodon to reject the request).
async fn inject_mastodon_actor_into_emumet_cache(
    cfg: &super::config::ApE2eConfig,
    user: &mut MastodonUser,
) {
    use super::account_helper::e2e_http_client;

    // Query the public key from Mastodon's database.
    // The actor URL from `user.actor_url` (Mastodon's `uri` field) uses the
    // correct `/ap/users/{numeric_id}` format and is used as the cache key.
    let actor_id = user.actor_url.clone();
    let sql = format!(
        "SELECT public_key FROM accounts WHERE username='{username}'",
        username = user.username,
    );
    let output = tokio::process::Command::new("docker")
        .args([
            "exec",
            "emumet-ap-e2e-mastodon-db",
            "psql",
            "-U",
            "mastodon",
            "-d",
            "mastodon",
            "-t",
            "-A",
            "-c",
            &sql,
        ])
        .output()
        .await
        .expect("failed to execute docker exec psql");
    assert!(
        output.status.success(),
        "docker exec psql failed (exit={}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let public_key_pem = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(
        !public_key_pem.is_empty(),
        "public_key from database is empty"
    );

    // The inbox URL uses the federation port 18443 for outbound delivery.
    let inbox_url = actor_id.replace(
        "//mastodon.127.0.0.1.nip.io/",
        "//mastodon.127.0.0.1.nip.io:18443/",
    ) + "/inbox";

    let emumet_client = e2e_http_client();
    let test_token =
        std::env::var("EMUMET_TEST_MODE_TOKEN").expect("EMUMET_TEST_MODE_TOKEN must be set");

    let cache_key_url = format!(
        "{}/__test__/cache-actor-key",
        cfg.server_base_url.trim_end_matches('/')
    );
    let cache_key_resp = emumet_client
        .post(&cache_key_url)
        .header("X-Emumet-Test-Token", &test_token)
        .json(&serde_json::json!({
            "key_id": format!("{}#main-key", actor_id),
            "public_key_pem": public_key_pem,
            "owner": actor_id,
        }))
        .send()
        .await
        .expect("failed to send cache-actor-key request");
    assert_eq!(
        cache_key_resp.status(),
        reqwest::StatusCode::NO_CONTENT,
        "cache-actor-key should return 204"
    );

    let cache_remote_actor_url = format!(
        "{}/__test__/cache-remote-actor",
        cfg.server_base_url.trim_end_matches('/')
    );
    let cache_actor_resp = emumet_client
        .post(&cache_remote_actor_url)
        .header("X-Emumet-Test-Token", &test_token)
        .json(&serde_json::json!({
            "actor_url": actor_id,
            "username": user.username,
            "inbox_url": inbox_url,
            "public_key_pem": public_key_pem,
        }))
        .send()
        .await
        .expect("failed to send cache-remote-actor request");
    assert_eq!(
        cache_actor_resp.status(),
        reqwest::StatusCode::NO_CONTENT,
        "cache-remote-actor should return 204"
    );
}

// -- URL builders -------------------------------------------------

pub fn emumet_actor_url(public_base_url: &str, account_id: &str) -> String {
    format!(
        "{}/ap/accounts/{}",
        public_base_url.trim_end_matches('/'),
        account_id
    )
}

// -- JSON helpers --------------------------------------------------

pub fn extract_remote_account_id(account: &serde_json::Value) -> String {
    account["id"]
        .as_str()
        .expect("search account result missing 'id'")
        .to_string()
}

// -- Polling helpers -----------------------------------------------

pub async fn wait_for_mastodon_followers_contains(
    client: &super::mastodon::MastodonClient,
    local_account_id: &str,
    token: &str,
    expected_actor_url: &str,
) -> bool {
    for _ in 1..=60 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if let Ok(followers) = client.get_followers(local_account_id, token).await {
            if followers.iter().any(|follower| {
                follower["url"]
                    .as_str()
                    .is_some_and(|u| u == expected_actor_url)
                    || follower["uri"]
                        .as_str()
                        .is_some_and(|u| u == expected_actor_url)
            }) {
                return true;
            }
        }
    }
    false
}

pub async fn wait_for_mastodon_following_contains(
    client: &super::mastodon::MastodonClient,
    local_account_id: &str,
    token: &str,
    expected_actor_url: &str,
) -> bool {
    for _ in 1..=60 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if let Ok(following) = client.get_following(local_account_id, token).await {
            if following.iter().any(|followee| {
                followee["url"]
                    .as_str()
                    .is_some_and(|u| u == expected_actor_url)
                    || followee["uri"]
                        .as_str()
                        .is_some_and(|u| u == expected_actor_url)
            }) {
                return true;
            }
        }
    }
    false
}

pub async fn wait_for_emumet_collection_count(
    base_url: &str,
    account_id: &str,
    collection: &str,
    min_items: u64,
) -> bool {
    // Emumet processes follows synchronously during the request, so a
    // shorter poll interval is acceptable here.
    for _ in 1..=60 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let col = fetch_collection(base_url, account_id, collection).await;
        if col["totalItems"].as_u64().unwrap_or(0) >= min_items {
            return true;
        }
    }
    false
}

pub async fn wait_for_mastodon_note_uri(
    client: &super::mastodon::MastodonClient,
    token: &str,
    note_uri: &str,
) -> Result<(), String> {
    let mut last_error: Option<String> = None;
    for i in 1..=60 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        match client.public_timeline_contains_uri(token, note_uri).await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(e) => {
                last_error = Some(e.to_string());
                tracing::warn!(
                    attempt = i,
                    error = %last_error.as_deref().unwrap_or("unknown"),
                    "Mastodon public timeline API error (retrying)"
                );
            }
        }
    }
    let detail = match last_error {
        Some(err) => format!("timeout after 60 polls, last API error: {err}"),
        None => "timeout after 60 polls, note never appeared (no API errors)".to_string(),
    };
    Err(detail)
}

/// Poll a Mastodon account's own statuses until a status with the given
/// ActivityPub `uri` appears (or timeout after ~60 s).
pub async fn wait_for_account_statuses_contains_uri(
    client: &super::mastodon::MastodonClient,
    account_id: &str,
    token: &str,
    note_uri: &str,
) -> Result<(), String> {
    let mut last_error: Option<String> = None;
    for i in 1..=60 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        match client.get_account_statuses(account_id, token).await {
            Ok(statuses) => {
                if statuses
                    .iter()
                    .any(|s| s.get("uri").and_then(|v| v.as_str()) == Some(note_uri))
                {
                    return Ok(());
                }
            }
            Err(e) => {
                last_error = Some(e.to_string());
                tracing::warn!(
                    attempt = i,
                    error = %last_error.as_deref().unwrap_or("unknown"),
                    "Mastodon account statuses API error (retrying)"
                );
            }
        }
    }
    let detail = match last_error {
        Some(err) => format!("timeout after 60 polls, last API error: {err}"),
        None => "timeout after 60 polls, note never appeared in account statuses (no API errors)"
            .to_string(),
    };
    Err(detail)
}
