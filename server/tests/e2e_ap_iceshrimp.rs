//! ActivityPub Federation E2E Tests — Iceshrimp Scenario (S7)
//!
//! This test verifies cross-instance follow between Emumet and a real Iceshrimp
//! instance running in a compose profile (compose.yml + compose.ap-e2e.yml).
//!
//! Run with:
//!   EMUMET_E2E_EXTERNAL_SERVER=1 cargo test -p server \
//!     --test e2e_ap_iceshrimp -- --ignored --test-threads=1 --nocapture

mod support;

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
    let emumet_account_id = support::account_helper::setup_test_account().await;

    // ── 3. Create Iceshrimp client ────────────────────────────────
    let iceshrimp_base_url = std::env::var("ICESHRIMP_BASE_URL")
        .unwrap_or_else(|_| "https://iceshrimp.127.0.0.1.nip.io:8443".to_string());
    let ics = support::iceshrimp::IceshrimpClient::new(&iceshrimp_base_url);

    // ── 4. Sign up on Iceshrimp ───────────────────────────────────
    let unique_suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let ics_username = format!("e2e-alice-{unique_suffix}");

    let signup_result = ics
        .signup(&ics_username, "test-pass")
        .await
        .expect("Iceshrimp signup failed — is compose running?");
    let ics_token = signup_result["i"]
        .as_str()
        .expect("signup response missing 'i' token")
        .to_string();
    let local_iceshrimp_user_id = signup_result["id"]
        .as_str()
        .expect("signup response missing local user 'id'")
        .to_string();

    // ── 5. Resolve Emumet account via WebFinger / AP ──────────────
    let emumet_acct = format!("acct:{emumet_account_id}@emumet.127.0.0.1.nip.io:8443");
    let resolve_result = ics
        .resolve_remote_user(&emumet_acct, &ics_token)
        .await
        .expect("failed to resolve Emumet account from Iceshrimp");
    let remote_user_id = resolve_result["id"]
        .as_str()
        .expect("resolve response missing 'id'")
        .to_string();

    // ── 6. Iceshrimp user follows the Emumet account ──────────────
    ics.follow_user(&remote_user_id, &ics_token)
        .await
        .expect("failed to follow Emumet account from Iceshrimp");

    // ── 7. Wait for and verify Emumet's followers collection ──────
    let followers_client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("failed to build followers client");
    let followers_url = format!(
        "{}/accounts/{emumet_account_id}/followers",
        cfg.public_base_url
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
    let following = ics
        .get_following(&local_iceshrimp_user_id, &ics_token)
        .await
        .expect("failed to get Iceshrimp following list");
    assert!(
        !following.is_empty(),
        "Iceshrimp user should be following the Emumet account, got empty list"
    );
}
