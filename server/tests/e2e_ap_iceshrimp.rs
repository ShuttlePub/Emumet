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
