//! ActivityPub Federation E2E Tests — Iceshrimp Scenario (S7)
//!
//! This test verifies cross-instance follow between Emumet and a real Iceshrimp
//! instance running in a compose profile (compose.yml + compose.ap-e2e.yml).
//!
//! Run with:
//!   EMUMET_E2E_EXTERNAL_SERVER=1 cargo test -p server \
//!     --test e2e_ap_iceshrimp -- --ignored --test-threads=1 --nocapture

#[allow(dead_code)]
mod support;

use support::account_helper::e2e_http_client;
use support::config::ap_e2e_config;
use support::db;

#[tokio::test]
#[ignore]
async fn iceshrimp_follows_emumet_account() {
    // This test requires the full compose environment to be running.
    if std::env::var("EMUMET_E2E_EXTERNAL_SERVER").as_deref() != Ok("1") {
        panic!(
            "S7 Iceshrimp E2E test requires EMUMET_E2E_EXTERNAL_SERVER=1\n\
             This test needs the compose environment (compose.yml + compose.ap-e2e.yml)\n\
             and Emumet running in test-mode."
        );
    }

    // ── 1. Setup ──────────────────────────────────────────────────
    let cfg = ap_e2e_config();
    db::reset_test_data().await;

    // ── 2. Create an account on Emumet ────────────────────────────
    let emumet_account = support::account_helper::setup_test_account_details().await;

    // ── 3. Create Iceshrimp client ────────────────────────────────
    let iceshrimp_base_url = std::env::var("ICESHRIMP_BASE_URL")
        .expect("ICESHRIMP_BASE_URL must be set for S7 Iceshrimp test");
    let ics = support::iceshrimp::IceshrimpClient::new(&iceshrimp_base_url);

    // ── 4. Sign up on Iceshrimp ───────────────────────────────────
    let unique_suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let ics_username = format!("e2e_{unique_suffix}");

    let signup_result = ics
        .signup(&ics_username, "test-pass")
        .await
        .expect("Iceshrimp signup failed — is compose running?");
    let ics_token = signup_result["token"]
        .as_str()
        .expect("signup response missing token")
        .to_string();
    let local_iceshrimp_user_id = signup_result["id"]
        .as_str()
        .expect("signup response missing local user 'id'")
        .to_string();

    // ── 4b. Inject Iceshrimp user's public key into Emumet cache ──
    // Iceshrimp v2026.5.1 returns 401 for its ActivityPub actor endpoint
    // and does not expose the publicKey through any REST API. The key is
    // stored in the `user_keypair` database table but not included in API
    // responses (only in the ActivityPub actor document rendered on fetch).
    //
    // We generate a known RSA key pair, inject it into Iceshrimp's
    // database via docker exec psql, and inject the public key into
    // Emumet's HTTP Signature verifier cache. This gives both sides a
    // matching key pair without needing to fetch Iceshrimp's ActivityPub
    // actor endpoint (which returns 401).
    use rand::rngs::OsRng;
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
    use rsa::RsaPrivateKey;
    use std::io::Write;

    let mut rng = OsRng;
    let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("failed to generate RSA key pair");
    let public_key = rsa::RsaPublicKey::from(&private_key);
    let private_key_pem = private_key
        .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
        .expect("failed to PEM-encode private key")
        .to_string();
    let ics_public_key_pem = public_key
        .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
        .expect("failed to PEM-encode public key");

    let sql = format!(
        r#"UPDATE user_keypair SET "publicKey" = '{public_pem}', "privateKey" = '{private_pem}' WHERE "userId" = '{uid}';"#,
        public_pem = &ics_public_key_pem.replace('\'', r"'\''"),
        private_pem = &private_key_pem.replace('\'', r"'\''"),
        uid = local_iceshrimp_user_id,
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
    let actor_url = format!(
        "{}/users/{}",
        iceshrimp_base_url.trim_end_matches('/'),
        local_iceshrimp_user_id,
    );
    emumet_client
        .post(&cache_key_url)
        .header("X-Emumet-Test-Token", &test_token)
        .json(&serde_json::json!({
            "key_id": format!("{actor_url}#main-key"),
            "public_key_pem": ics_public_key_pem,
            "owner": actor_url,
        }))
        .send()
        .await
        .expect("failed to inject actor key into Emumet cache");

    // ── 4c. Inject Iceshrimp actor data into Emumet resolver cache ─
    // Iceshrimp v2026.5.1 returns 401 for its ActivityPub actor endpoint
    // when accessed without authentication. We inject the actor data
    // (username, inbox URL, public key) into the test cache so that
    // Emumet's resolve_remote_actor can use it without making an HTTP request.
    let cache_remote_actor_url = format!(
        "{}/__test__/cache-remote-actor",
        cfg.server_base_url.trim_end_matches('/')
    );
    let ics_inbox_url = format!("{actor_url}/inbox");
    emumet_client
        .post(&cache_remote_actor_url)
        .header("X-Emumet-Test-Token", &test_token)
        .json(&serde_json::json!({
            "actor_url": actor_url,
            "username": ics_username,
            "inbox_url": ics_inbox_url,
            "public_key_pem": ics_public_key_pem,
        }))
        .send()
        .await
        .expect("failed to inject remote actor data into Emumet cache");

    // ── 5. Resolve Emumet account via Actor URL ───────────────────
    // Iceshrimp's ap/show node-fetch doesn't support acct: URIs,
    // so we pass the actor URL directly.
    let public_base_url = cfg.public_base_url.trim_end_matches('/');
    let emumet_actor_url = format!("{public_base_url}/accounts/{}", emumet_account.id);
    let resolve_result = ics
        .resolve_remote_user(&emumet_actor_url, &ics_token)
        .await
        .expect("failed to resolve Emumet account from Iceshrimp");
    // /api/ap/show returns { type, object } where object contains the remote actor
    let remote_object = resolve_result["object"]
        .as_object()
        .or_else(|| resolve_result.as_object())
        .expect("resolve response should be an object with 'object' field");
    let remote_user_id = remote_object
        .get("id")
        .and_then(|v| v.as_str())
        .or_else(|| resolve_result.get("id").and_then(|v| v.as_str()))
        .or_else(|| {
            resolve_result
                .get("object")
                .and_then(|v| v.get("id").and_then(|v| v.as_str()))
        })
        .expect("resolve response missing actor 'id'")
        .to_string();

    // ── 6. Iceshrimp user follows the Emumet account ──────────────
    ics.follow_user(&remote_user_id, &ics_token)
        .await
        .expect("failed to follow Emumet account from Iceshrimp");

    // ── 7. Wait for and verify Emumet's followers collection ──────
    let followers_client = e2e_http_client();
    // Use server_base_url (HTTP localhost) for direct server access;
    // public_base_url uses HTTPS which requires port 443 mapping from the host.
    let followers_url = format!(
        "{}/accounts/{}/followers",
        cfg.server_base_url, emumet_account.id
    );

    let mut followers_verified = false;
    for _ in 1..=30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Ok(resp) = followers_client
            .get(&followers_url)
            .header("accept", "application/activity+json")
            .send()
            .await
        {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                if body["totalItems"].as_u64().unwrap_or(0) >= 1 {
                    followers_verified = true;
                    break;
                }
            }
        }
    }
    assert!(
        followers_verified,
        "Emumet followers should include the Iceshrimp user within timeout"
    );

    // ── 9. Verify Iceshrimp's following list ─────────────────────
    let mut following_verified = false;
    for _ in 1..=30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Ok(following) = ics
            .get_following(&local_iceshrimp_user_id, &ics_token)
            .await
        {
            if following.iter().any(|entry| {
                let followee = &entry["followee"];
                followee["uri"]
                    .as_str()
                    .is_some_and(|u| u == emumet_actor_url)
                    || followee["id"]
                        .as_str()
                        .is_some_and(|i| i == emumet_actor_url)
            }) {
                following_verified = true;
                break;
            }
        }
    }

    assert!(
        following_verified,
        "Iceshrimp user should be following the Emumet account (actor URL: {emumet_actor_url})"
    );
}
