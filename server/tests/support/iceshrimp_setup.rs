//! Shared setup helpers for Iceshrimp ActivityPub E2E tests.
//!
//! Extracted from the S7 test to reduce duplication across S7, S8, and S9.

use std::io::Write;

use rand::rngs::OsRng;
use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
use rsa::RsaPrivateKey;

use super::account_helper::{e2e_http_client, fetch_collection};
use super::config::ap_e2e_config;

// ── Types ─────────────────────────────────────────────────────────

/// Represents a test user created on the Iceshrimp instance.
pub struct IceshrimpUser {
    pub username: String,
    pub token: String,
    pub user_id: String,
    pub actor_url: String,
    pub inbox_url: String,
    pub public_key_pem: String,
}

/// Convenience bundle holding both the Iceshrimp client and the test user.
pub struct IceshrimpFixture {
    pub client: super::iceshrimp::IceshrimpClient,
    pub user: IceshrimpUser,
}

// ── Guards ────────────────────────────────────────────────────────

/// Panic unless the AP E2E external-server environment is active.
pub fn require_ap_e2e_external_server(test_name: &str) {
    if std::env::var("EMUMET_E2E_EXTERNAL_SERVER").as_deref() != Ok("1") {
        panic!(
            "{test_name} requires EMUMET_E2E_EXTERNAL_SERVER=1\n\
             This test needs the compose environment (compose.yml + compose.ap-e2e.yml)\n\
             and Emumet running in test-mode."
        );
    }
}

// ─── Setup helpers ───────────────────────────────────────────────

/// Sign up a new user on the Iceshrimp instance and return a fixture.
pub async fn signup_iceshrimp_user() -> IceshrimpFixture {
    let iceshrimp_base_url = std::env::var("ICESHRIMP_BASE_URL")
        .expect("ICESHRIMP_BASE_URL must be set for Iceshrimp E2E test");
    let client = super::iceshrimp::IceshrimpClient::new(&iceshrimp_base_url);

    let unique_suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let username = format!("e2e_{unique_suffix}");

    let signup_result = client
        .signup(&username, "test-pass")
        .await
        .expect("Iceshrimp signup failed — is compose running?");
    let token = signup_result["token"]
        .as_str()
        .expect("signup response missing token")
        .to_string();
    let user_id = signup_result["id"]
        .as_str()
        .expect("signup response missing local user 'id'")
        .to_string();

    let base = iceshrimp_base_url.trim_end_matches('/');
    let actor_url = format!("{base}/users/{user_id}");
    let inbox_url = format!("{actor_url}/inbox");

    IceshrimpFixture {
        client,
        user: IceshrimpUser {
            username,
            token,
            user_id,
            actor_url,
            inbox_url,
            public_key_pem: String::new(),
        },
    }
}

/// Generate an RSA key pair, inject it into Iceshrimp's database via
/// `docker exec psql`, and cache the public key + actor data in
/// Emumet's test-mode endpoints.
///
/// After this call `user.public_key_pem` is populated with the generated
/// public key so callers can reference it for verification.
pub async fn inject_iceshrimp_actor_into_emumet_cache(
    cfg: &super::config::ApE2eConfig,
    user: &mut IceshrimpUser,
) {
    let mut rng = OsRng;
    let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("failed to generate RSA key pair");
    let public_key = rsa::RsaPublicKey::from(&private_key);
    let private_key_pem = private_key
        .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
        .expect("failed to PEM-encode private key")
        .to_string();
    let public_key_pem = public_key
        .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
        .expect("failed to PEM-encode public key");

    let sql = format!(
        r#"UPDATE user_keypair SET "publicKey" = '{public_pem}', "privateKey" = '{private_pem}' WHERE "userId" = '{uid}';"#,
        public_pem = &public_key_pem.replace('\'', r"'\''"),
        private_pem = &private_key_pem.replace('\'', r"'\''"),
        uid = user.user_id,
    );
    let docker_status = std::process::Command::new("docker")
        .args([
            "exec",
            "-i",
            "emumet-ap-e2e-iceshrimp-db",
            "psql",
            "-U",
            "iceshrimp",
            "-d",
            "iceshrimp",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .and_then(|mut child| {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(sql.as_bytes())?;
            }
            child.wait()
        })
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("docker exec psql exited with non-zero status: {status}"),
                ))
            }
        });
    docker_status
        .expect("failed to inject RSA key pair into Iceshrimp user_keypair via docker exec psql");

    let cache_key_url = format!(
        "{}/__test__/cache-actor-key",
        cfg.server_base_url.trim_end_matches('/')
    );
    let emumet_client = e2e_http_client();
    let test_token =
        std::env::var("EMUMET_TEST_MODE_TOKEN").expect("EMUMET_TEST_MODE_TOKEN must be set");
    let cache_key_resp = emumet_client
        .post(&cache_key_url)
        .header("X-Emumet-Test-Token", &test_token)
        .json(&serde_json::json!({
            "key_id": format!("{}#main-key", user.actor_url),
            "public_key_pem": public_key_pem,
            "owner": user.actor_url,
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
            "actor_url": user.actor_url,
            "username": user.username,
            "inbox_url": user.inbox_url,
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

    user.public_key_pem = public_key_pem;
}

/// Full one-shot: sign up an Iceshrimp user AND inject its actor data
/// into Emumet's caches.
pub async fn setup_iceshrimp_remote_actor() -> (super::config::ApE2eConfig, IceshrimpFixture) {
    let cfg = ap_e2e_config();
    let mut fixture = signup_iceshrimp_user().await;
    inject_iceshrimp_actor_into_emumet_cache(&cfg, &mut fixture.user).await;
    (cfg, fixture)
}

// ── URL builders ──────────────────────────────────────────────────

/// Build the ActivityPub actor URL for an Emumet account.
pub fn emumet_actor_url(public_base_url: &str, account_id: &str) -> String {
    format!(
        "{}/accounts/{}",
        public_base_url.trim_end_matches('/'),
        account_id
    )
}

// ── Polling helpers ───────────────────────────────────────────────

/// Poll Iceshrimp's followers list until a follower with the expected
/// actor URL appears (or timeout after ~15 s).
pub async fn wait_for_iceshrimp_followers_contains(
    client: &super::iceshrimp::IceshrimpClient,
    local_iceshrimp_user_id: &str,
    token: &str,
    expected_actor_url: &str,
) -> bool {
    for _ in 1..=30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Ok(followers) = client.get_followers(local_iceshrimp_user_id, token).await {
            if followers.iter().any(|entry| {
                let follower = &entry["follower"];
                follower["uri"]
                    .as_str()
                    .is_some_and(|u| u == expected_actor_url)
                    || follower["id"]
                        .as_str()
                        .is_some_and(|i| i == expected_actor_url)
            }) {
                return true;
            }
        }
    }
    false
}

/// Poll Iceshrimp's following list until a followee with the expected
/// actor URL appears (or timeout after ~15 s).
pub async fn wait_for_iceshrimp_following_contains(
    client: &super::iceshrimp::IceshrimpClient,
    local_iceshrimp_user_id: &str,
    token: &str,
    expected_actor_url: &str,
) -> bool {
    for _ in 1..=30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Ok(following) = client.get_following(local_iceshrimp_user_id, token).await {
            if following.iter().any(|entry| {
                let followee = &entry["followee"];
                followee["uri"]
                    .as_str()
                    .is_some_and(|u| u == expected_actor_url)
                    || followee["id"]
                        .as_str()
                        .is_some_and(|i| i == expected_actor_url)
            }) {
                return true;
            }
        }
    }
    false
}

/// Poll an Emumet ActivityPub collection (followers / following) until
/// `totalItems >= min_items` (or timeout after ~15 s).
pub async fn wait_for_emumet_collection_count(
    base_url: &str,
    account_id: &str,
    collection: &str,
    min_items: u64,
) -> bool {
    for _ in 1..=30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let col = fetch_collection(base_url, account_id, collection).await;
        if col["totalItems"].as_u64().unwrap_or(0) >= min_items {
            return true;
        }
    }
    false
}

/// Poll Iceshrimp's global timeline until a note with the given
/// ActivityPub `uri` appears (or timeout after ~15 s).
///
/// Returns `Ok(())` on success.  Returns `Err(msg)` on timeout with the
/// last API error (if any) or no-error count.
pub async fn wait_for_iceshrimp_global_note_uri(
    client: &super::iceshrimp::IceshrimpClient,
    token: &str,
    note_uri: &str,
) -> Result<(), String> {
    let mut last_error: Option<String> = None;
    for i in 1..=30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        match client.global_timeline_contains_uri(token, note_uri).await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(e) => {
                last_error = Some(e.to_string());
                tracing::warn!(
                    attempt = i,
                    error = %last_error.as_deref().unwrap_or("unknown"),
                    "Iceshrimp global timeline API error (retrying)"
                );
            }
        }
    }
    let detail = match last_error {
        Some(err) => format!("timeout after 30 polls, last API error: {err}"),
        None => "timeout after 30 polls, note never appeared (no API errors)".to_string(),
    };
    Err(detail)
}

// ── JSON helpers ──────────────────────────────────────────────────

/// Extract the remote user ID from an Iceshrimp `/api/ap/show` response.
///
/// Iceshrimp's response has a nested shape: `{ type, object: { id, ... } }`
/// or flat `{ id, ... }`.  This tries both forms with a fallback chain.
pub fn extract_resolved_remote_user_id(resolve_result: &serde_json::Value) -> String {
    resolve_result
        .get("object")
        .and_then(|obj| obj.get("id").and_then(|v| v.as_str()))
        .or_else(|| resolve_result.get("id").and_then(|v| v.as_str()))
        .or_else(|| {
            resolve_result
                .as_object()
                .and_then(|_| resolve_result.get("id").and_then(|v| v.as_str()))
        })
        .expect("resolve response missing actor 'id'")
        .to_string()
}
