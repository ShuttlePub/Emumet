//! ActivityPub Federation E2E Tests — Iceshrimp Scenarios (S7-S9)
//!
//! These tests verify cross-instance ActivityPub federation between Emumet
//! and a real Iceshrimp instance running in a compose profile
//! (compose.yml + compose.ap-e2e.yml).
//!
//! | Test | Direction | What is verified |
//! |------|-----------|-----------------|
//! | S7   | Iceshrimp → Emumet | Iceshrimp follows Emumet, both collections update |
//! | S8   | Emumet → Iceshrimp | Emumet follows Iceshrimp, both collections update |
//! | S9   | Emumet → Iceshrimp | Emumet signs and delivers a Create/Note activity |
//!
//! Run with:
//!   EMUMET_E2E_EXTERNAL_SERVER=1 cargo test -p server \
//!     --test e2e_ap_iceshrimp -- --ignored --test-threads=1 --nocapture

#[allow(dead_code)]
mod support;

use support::account_helper::{e2e_http_client, post_follow, setup_test_account_details};
use support::auth;
use support::config::ap_e2e_config;
use support::db;
use support::iceshrimp_setup;

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

// ── S7: Iceshrimp → Emumet Follow ────────────────────────────────

#[tokio::test]
#[ignore]
async fn iceshrimp_follows_emumet_account() {
    init_test_tracing();
    iceshrimp_setup::require_ap_e2e_external_server("S7");

    let cfg = ap_e2e_config();
    db::reset_test_data().await;

    let emumet_account = setup_test_account_details().await;
    let (_cfg, fixture) = iceshrimp_setup::setup_iceshrimp_remote_actor().await;

    let public_base_url = cfg.public_base_url.trim_end_matches('/');
    let emumet_actor_url = iceshrimp_setup::emumet_actor_url(public_base_url, &emumet_account.id);

    let resolve_result = fixture
        .client
        .resolve_remote_user(&emumet_actor_url, &fixture.user.token)
        .await
        .expect("failed to resolve Emumet account from Iceshrimp");

    let remote_user_id = iceshrimp_setup::extract_resolved_remote_user_id(&resolve_result);

    fixture
        .client
        .follow_user(&remote_user_id, &fixture.user.token)
        .await
        .expect("failed to follow Emumet account from Iceshrimp");

    assert!(
        iceshrimp_setup::wait_for_emumet_collection_count(
            &cfg.server_base_url,
            &emumet_account.id,
            "followers",
            1,
        )
        .await,
        "Emumet followers should include the Iceshrimp user within timeout"
    );

    assert!(
        iceshrimp_setup::wait_for_iceshrimp_following_contains(
            &fixture.client,
            &fixture.user.user_id,
            &fixture.user.token,
            &emumet_actor_url,
        )
        .await,
        "Iceshrimp user should be following the Emumet account"
    );
}

// ── S8: Emumet → Iceshrimp Follow ────────────────────────────────

#[tokio::test]
#[ignore]
async fn emumet_follows_iceshrimp_account() {
    init_test_tracing();
    iceshrimp_setup::require_ap_e2e_external_server("S8");

    let cfg = ap_e2e_config();
    db::reset_test_data().await;

    let jwt = auth::get_jwt_for_test_user().await;
    let emumet_account = setup_test_account_details().await;
    let (_cfg, fixture) = iceshrimp_setup::setup_iceshrimp_remote_actor().await;

    let follow_resp = post_follow(
        &jwt,
        &emumet_account.id,
        &cfg.server_base_url,
        &fixture.user.actor_url,
    )
    .await;

    assert!(
        follow_resp.status().is_success(),
        "Emumet follow request should succeed: {}",
        follow_resp.status()
    );

    assert!(
        iceshrimp_setup::wait_for_emumet_collection_count(
            &cfg.server_base_url,
            &emumet_account.id,
            "following",
            1,
        )
        .await,
        "Emumet following should include the Iceshrimp user within timeout"
    );

    let expected_actor_url = iceshrimp_setup::emumet_actor_url(
        cfg.public_base_url.trim_end_matches('/'),
        &emumet_account.id,
    );
    assert!(
        iceshrimp_setup::wait_for_iceshrimp_followers_contains(
            &fixture.client,
            &fixture.user.user_id,
            &fixture.user.token,
            &expected_actor_url,
        )
        .await,
        "Iceshrimp followers should include the Emumet account within timeout"
    );
}

// ── S9: Emumet → Iceshrimp Signed Create/Note ────────────────────

#[tokio::test]
#[ignore]
async fn emumet_sends_signed_create_note_to_iceshrimp() {
    init_test_tracing();
    iceshrimp_setup::require_ap_e2e_external_server("S9");

    let cfg = ap_e2e_config();
    db::reset_test_data().await;

    let jwt = auth::get_jwt_for_test_user().await;
    let emumet_account = setup_test_account_details().await;
    let (_cfg, fixture) = iceshrimp_setup::setup_iceshrimp_remote_actor().await;

    let public_base_url = cfg.public_base_url.trim_end_matches('/');
    let actor_url = format!("{public_base_url}/accounts/{}", emumet_account.id);
    let followers_url = format!("{actor_url}/followers");

    // ── 1. Establish follow (Iceshrimp → Emumet) ───────────────────
    // So the delivered note appears in Iceshrimp's timeline.
    let resolve_result = fixture
        .client
        .resolve_remote_user(&actor_url, &fixture.user.token)
        .await
        .expect("failed to resolve Emumet account from Iceshrimp");
    let remote_user_id = iceshrimp_setup::extract_resolved_remote_user_id(&resolve_result);

    fixture
        .client
        .follow_user(&remote_user_id, &fixture.user.token)
        .await
        .expect("failed to follow Emumet account from Iceshrimp");

    assert!(
        iceshrimp_setup::wait_for_emumet_collection_count(
            &cfg.server_base_url,
            &emumet_account.id,
            "followers",
            1,
        )
        .await,
        "S9: Emumet should have the Iceshrimp follower before posting"
    );

    // Also verify Iceshrimp's following list shows the Emumet account
    // before proceeding with delivery.
    assert!(
        iceshrimp_setup::wait_for_iceshrimp_following_contains(
            &fixture.client,
            &fixture.user.user_id,
            &fixture.user.token,
            &actor_url,
        )
        .await,
        "S9: Iceshrimp should be following the Emumet account before posting"
    );

    // ── 2. Build the Create/Note activity ──────────────────────────
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

    // ── 3. Compute HTTP headers for signing ────────────────────────
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
    let base64_body = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &body);

    // ── 4. Sign via Emumet's /accounts/{id}/sign endpoint ──────────
    let sign_resp = e2e_http_client()
        .post(format!(
            "{}/accounts/{}/sign",
            cfg.server_base_url, emumet_account.id
        ))
        .bearer_auth(&jwt)
        .json(&serde_json::json!({
            "method": "POST",
            "url": inbox_url,
            "headers": {
                "host": host_header,
                "date": date,
                "digest": digest,
                "content-type": "application/activity+json"
            },
            "body": base64_body,
        }))
        .send()
        .await
        .expect("failed to sign Create/Note via /sign endpoint");

    assert!(
        sign_resp.status().is_success(),
        "Sign endpoint returned {}",
        sign_resp.status()
    );

    let sign_body: serde_json::Value = sign_resp
        .json()
        .await
        .expect("sign response should be valid JSON");
    let cavage = sign_body["cavage"]
        .as_object()
        .expect("sign response should contain 'cavage' object")
        .clone();

    // ── 5. Deliver the signed activity to Iceshrimp's inbox ────────
    let mut inbox_req = e2e_http_client()
        .post(&inbox_url)
        .header("host", &host_header)
        .header("date", &date)
        .header("digest", &digest)
        .header("content-type", "application/activity+json");

    for (name, value) in &cavage {
        let lower = name.to_ascii_lowercase();
        if lower != "host" && lower != "date" && lower != "digest" && lower != "content-type" {
            if let Some(val) = value.as_str() {
                inbox_req = inbox_req.header(name.as_str(), val);
            }
        }
    }

    let delivery_resp = inbox_req
        .body(body)
        .send()
        .await
        .expect("signed Create/Note POST to Iceshrimp inbox failed");

    assert!(
        delivery_resp.status().is_success(),
        "Iceshrimp inbox should accept the signed Create/Note: {}",
        delivery_resp.status()
    );

    // ── 6. Verify note appears in Iceshrimp's global timeline ──────
    iceshrimp_setup::wait_for_iceshrimp_global_note_uri(
        &fixture.client,
        &fixture.user.token,
        &note_id,
    )
    .await
    .unwrap_or_else(|err| {
        panic!(
            "Iceshrimp global timeline should contain the delivered note \
             (uri: {note_id}). Reason: {err}"
        )
    });

    // Note: The Note `id` uses a URI path (`/statuses/{uuid}`) that is not
    // currently a dereferenceable endpoint on Emumet.  Iceshrimp stores the
    // embedded Note with this URI as its `uri` field without immediately
    // dereferencing it.  If Iceshrimp changes to require a resolvable object
    // endpoint, the Note `id` must be updated to a real URL.
}
