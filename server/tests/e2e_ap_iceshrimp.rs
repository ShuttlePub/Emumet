//! ActivityPub Federation E2E Tests — Iceshrimp Scenarios (S7-S9)
//!
//! These tests verify cross-instance ActivityPub federation between Emumet
//! and a real Iceshrimp instance running in a compose profile
//! (compose.yml + compose.ap-e2e.yml).
//!
//! S7-S10 are combined into a single test (`iceshrimp_full_federation_scenario`)
//! so the final database state is available for manual inspection when run
//! with `EMUMET_E2E_PAUSE_BEFORE_CLEANUP=1`.
//!
//! | Step | Direction | What is verified |
//! |------|-----------|-----------------|
//! | S7   | Iceshrimp → Emumet | Iceshrimp follows Emumet, both collections update |
//! | S8   | Emumet → Iceshrimp | Emumet follows Iceshrimp, both collections update |
//! | S9   | Emumet → Iceshrimp | Emumet signs and delivers a Create/Note activity |
//! | S10  | Emumet → Iceshrimp | Emumet blocks/unblocks Iceshrimp, follows removed both sides |
//!
//! Run with:
//!   EMUMET_E2E_EXTERNAL_SERVER=1 cargo test -p server \
//!     --test e2e_ap_iceshrimp -- --ignored --test-threads=1 --nocapture

#[allow(dead_code)]
mod support;

use kernel::interfaces::http_signing::HttpSigningRequest;
use support::account_helper::{
    delete_block, e2e_http_client, get_relations, post_block, post_follow,
    setup_test_account_details,
};
use support::auth;
use support::config::ap_e2e_config;
use support::db;
use support::iceshrimp_setup;
use support::profile_signer;

fn init_test_tracing() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "e2e_ap_iceshrimp=info".into()),
            )
            .with_test_writer()
            .init();
    });
}

// ── S7-S9: Combined Federation Scenario ────────────────────────────────
// Iceshrimp → Emumet follow → Emumet → Iceshrimp follow → signed Create/Note

#[tokio::test]
#[ignore]
async fn iceshrimp_full_federation_scenario() {
    init_test_tracing();
    iceshrimp_setup::require_ap_e2e_external_server("S7-S9");

    let cfg = ap_e2e_config();
    db::reset_test_data().await;

    let jwt = auth::get_jwt_for_test_user().await;
    let emumet_account = setup_test_account_details().await;
    let (_cfg, fixture) = iceshrimp_setup::setup_iceshrimp_remote_actor().await;

    tracing::info!(
        iceshrimp_base_url = %fixture.client.base_url,
        iceshrimp_username = %fixture.user.username,
        iceshrimp_password = "test-pass",
        iceshrimp_user_id = %fixture.user.user_id,
        emumet_account_id = %emumet_account.id,
        emumet_account_name = %emumet_account.name,
        "S7-S9: combined federation scenario — account credentials for manual verification"
    );

    let public_base_url = cfg.public_base_url.trim_end_matches('/');
    let emumet_actor_url =
        iceshrimp_setup::emumet_actor_url(public_base_url, &emumet_account.profile_id);

    // ── S7: Iceshrimp → Emumet Follow ────────────────────────────────

    let resolve_result = fixture
        .client
        .resolve_remote_user(&emumet_actor_url, &fixture.user.token)
        .await
        .expect("S7: failed to resolve Emumet account from Iceshrimp");
    let remote_user_id = iceshrimp_setup::extract_resolved_remote_user_id(&resolve_result);

    fixture
        .client
        .follow_user(&remote_user_id, &fixture.user.token)
        .await
        .expect("S7: failed to follow Emumet account from Iceshrimp");

    assert!(
        iceshrimp_setup::wait_for_emumet_collection_count(
            &cfg.server_base_url,
            &jwt,
            &emumet_account.profile_id,
            "followers",
            1,
        )
        .await,
        "S7: Emumet followers should include the Iceshrimp user within timeout"
    );

    assert!(
        iceshrimp_setup::wait_for_iceshrimp_following_contains(
            &fixture.client,
            &fixture.user.user_id,
            &fixture.user.token,
            &emumet_actor_url,
        )
        .await,
        "S7: Iceshrimp user should be following the Emumet account"
    );

    // ── S8: Emumet → Iceshrimp Follow ────────────────────────────────

    let follow_resp = post_follow(
        &jwt,
        &emumet_account.profile_id,
        &cfg.server_base_url,
        &fixture.user.actor_url,
    )
    .await;

    assert_eq!(
        follow_resp.status(),
        reqwest::StatusCode::CREATED,
        "S8: Emumet follow request should succeed"
    );

    assert!(
        iceshrimp_setup::wait_for_emumet_collection_count(
            &cfg.server_base_url,
            &jwt,
            &emumet_account.profile_id,
            "following",
            1,
        )
        .await,
        "S8: Emumet following should include the Iceshrimp user within timeout"
    );

    let expected_actor_url = iceshrimp_setup::emumet_actor_url(
        cfg.public_base_url.trim_end_matches('/'),
        &emumet_account.profile_id,
    );
    assert!(
        iceshrimp_setup::wait_for_iceshrimp_followers_contains(
            &fixture.client,
            &fixture.user.user_id,
            &fixture.user.token,
            &expected_actor_url,
        )
        .await,
        "S8: Iceshrimp followers should include the Emumet account within timeout"
    );

    // ── S9: Emumet → Iceshrimp Signed Create/Note ────────────────────

    let actor_url = format!(
        "{public_base_url}/ap/profiles/{}",
        emumet_account.profile_id
    );
    let followers_url = format!("{actor_url}/followers");

    let note_id = format!("{actor_url}/statuses/{}", uuid::Uuid::new_v4());
    let activity_id = format!("{note_id}/activity");
    let inbox_url = fixture.user.inbox_url.clone();

    use time::format_description::well_known::Rfc3339;
    let published = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("format published timestamp");

    let create_activity = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": activity_id,
        "type": "Create",
        "actor": actor_url,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [followers_url],
        "object": {
            "id": note_id,
            "type": "Note",
            "attributedTo": actor_url,
            "content": "<p>Emumet AP E2E signed Create/Note</p>",
            "published": published,
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [followers_url]
        }
    });

    let body = serde_json::to_vec(&create_activity).expect("serialize Create/Note activity");

    use sha2::Digest;
    let digest = format!(
        "SHA-256={}",
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            sha2::Sha256::digest(&body),
        )
    );
    let date = httpdate::fmt_http_date(std::time::SystemTime::now());

    let inbox_parsed = url::Url::parse(&inbox_url).expect("invalid inbox URL");
    let inbox_host = inbox_parsed.host_str().expect("inbox URL must have a host");
    let host_header = match inbox_parsed.port() {
        Some(p) => format!("{inbox_host}:{p}"),
        None => inbox_host.to_string(),
    };
    // The server has no signing API: sign as the Profile with its own key (test fixture that
    // decrypts the stored key exactly like the delivery worker does).
    let cavage = profile_signer::sign_as_profile(
        &emumet_account.profile_id,
        &HttpSigningRequest {
            method: "POST".to_string(),
            url: inbox_url.clone(),
            headers: std::collections::HashMap::from([
                ("host".to_string(), host_header.clone()),
                ("date".to_string(), date.clone()),
                ("digest".to_string(), digest.clone()),
                (
                    "content-type".to_string(),
                    "application/activity+json".to_string(),
                ),
            ]),
            body: Some(body.clone()),
        },
    )
    .await;

    // Deliver the signed activity to Iceshrimp's inbox
    let mut inbox_req = e2e_http_client()
        .post(&inbox_url)
        .header("host", &host_header)
        .header("date", &date)
        .header("digest", &digest)
        .header("content-type", "application/activity+json");

    for (name, value) in &cavage {
        let lower = name.to_ascii_lowercase();
        if lower != "host" && lower != "date" && lower != "digest" && lower != "content-type" {
            inbox_req = inbox_req.header(name.as_str(), value.as_str());
        }
    }

    let delivery_resp = inbox_req
        .body(body)
        .send()
        .await
        .expect("S9: signed Create/Note POST to Iceshrimp inbox failed");

    assert!(
        delivery_resp.status().is_success(),
        "S9: Iceshrimp inbox should accept the signed Create/Note: {}",
        delivery_resp.status()
    );

    // Verify note appears in Iceshrimp's global timeline
    iceshrimp_setup::wait_for_iceshrimp_global_note_uri(
        &fixture.client,
        &fixture.user.token,
        &note_id,
    )
    .await
    .unwrap_or_else(|err| {
        panic!(
            "S9: Iceshrimp global timeline should contain the delivered note \
             (uri: {note_id}). Reason: {err}"
        )
    });

    // Note: The Note `id` uses a URI path (`/statuses/{uuid}`) that is not
    // currently a dereferenceable endpoint on Emumet.  Iceshrimp stores the
    // embedded Note with this URI as its `uri` field without immediately
    // dereferencing it.  If Iceshrimp changes to require a resolvable object
    // endpoint, the Note `id` must be updated to a real URL.

    // ── S10: Emumet → Iceshrimp individual block ─────────────────────
    // Both follow relations (Iceshrimp → Emumet from S7, Emumet → Iceshrimp from S8) are detached
    // at once: Iceshrimp learns it from Undo(Follow) / Reject(Follow). No ActivityPub Block is sent.

    let block_resp = post_block(
        &jwt,
        &emumet_account.profile_id,
        &cfg.server_base_url,
        &fixture.user.actor_url,
    )
    .await;
    assert_eq!(
        block_resp.status(),
        reqwest::StatusCode::CREATED,
        "S10: Emumet block request should succeed"
    );
    let block: serde_json::Value = block_resp
        .json()
        .await
        .expect("S10: block response should be valid JSON");
    let block_id = block["id"].as_str().expect("S10: block id").to_string();

    for list in ["following", "followers"] {
        assert!(
            iceshrimp_setup::wait_for_emumet_collection_count_at_most(
                &cfg.server_base_url,
                &jwt,
                &emumet_account.profile_id,
                list,
                0,
            )
            .await,
            "S10: Emumet {list} should be empty after blocking the Iceshrimp user"
        );
    }

    assert!(
        iceshrimp_setup::wait_for_iceshrimp_followers_absent(
            &fixture.client,
            &fixture.user.user_id,
            &fixture.user.token,
            &emumet_actor_url,
        )
        .await,
        "S10: Iceshrimp followers should drop the Emumet account after receiving Undo(Follow)"
    );

    assert!(
        iceshrimp_setup::wait_for_iceshrimp_following_absent(
            &fixture.client,
            &fixture.user.user_id,
            &fixture.user.token,
            &emumet_actor_url,
        )
        .await,
        "S10: Iceshrimp following should drop the Emumet account after receiving Reject(Follow)"
    );

    assert!(
        iceshrimp_setup::wait_for_iceshrimp_block_absent(
            &fixture.client,
            &remote_user_id,
            &fixture.user.token,
        )
        .await,
        "S10: no ActivityPub Block is sent, so Iceshrimp must not report the Emumet account as blocking \
         the local user (users/show isBlocked == false)"
    );

    let unblock_resp = delete_block(
        &jwt,
        &emumet_account.profile_id,
        &cfg.server_base_url,
        &block_id,
    )
    .await;
    assert_eq!(
        unblock_resp.status(),
        reqwest::StatusCode::NO_CONTENT,
        "S10: Emumet unblock request should return 204"
    );
    for list in ["following", "followers"] {
        let relations =
            get_relations(&jwt, &emumet_account.profile_id, &cfg.server_base_url, list).await;
        assert_eq!(
            relations["items"].as_array().map(Vec::len),
            Some(0),
            "S10: removing the block must not restore the relation ({list})"
        );
    }
}
