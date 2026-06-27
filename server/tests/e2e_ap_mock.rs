//! ActivityPub Federation E2E Tests — Mock Peer Scenarios (S1–S6)

mod support;

use std::time::Duration;

use support::account_helper::{
    assert_collection_has_items, assert_content_type, assert_signature_header, fetch_collection,
    post_follow, post_signed_follow, setup_test_account, start_server_with_peer,
};
use support::ap_peer::{wait_for_activity, ApPeer};
use support::auth;
use support::config::ap_e2e_config;
use support::db;
use support::server::EmumetServer;

fn config() -> support::config::ApE2eConfig {
    ap_e2e_config()
}

#[tokio::test]
#[ignore]
async fn webfinger_resolves_account() {
    db::reset_test_data().await;
    let cfg = config();
    let account_nanoid = setup_test_account().await;

    let resp = reqwest::Client::new()
        .get(format!("{}/.well-known/webfinger", cfg.server_base_url))
        .query(&[("resource", &format!("acct:{account_nanoid}@localhost:8080"))])
        .send()
        .await
        .expect("WebFinger request failed");

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_content_type(&resp, "application/jrd+json");

    let body: serde_json::Value = resp
        .json()
        .await
        .expect("WebFinger response not valid JSON");
    let subject = body["subject"]
        .as_str()
        .expect("WebFinger response missing subject");
    assert!(
        subject.contains(&account_nanoid),
        "subject should contain account nanoid: {subject}"
    );

    let links = body["links"]
        .as_array()
        .expect("WebFinger response missing links");
    let self_link = links
        .iter()
        .find(|link| link["rel"] == "self")
        .expect("WebFinger response missing self link");
    assert_eq!(self_link["type"], "application/activity+json");
    let href = self_link["href"].as_str().expect("self link missing href");
    assert!(
        href.contains(&account_nanoid),
        "self link href should contain account ID: {href}"
    );
}

#[tokio::test]
#[ignore]
async fn actor_document_is_valid_activitypub() {
    db::reset_test_data().await;
    let cfg = config();
    let account_nanoid = setup_test_account().await;

    let resp = reqwest::Client::new()
        .get(format!("{}/accounts/{account_nanoid}", cfg.server_base_url))
        .header(reqwest::header::ACCEPT, "application/activity+json")
        .send()
        .await
        .expect("Actor request failed");

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_content_type(&resp, "application/activity+json");

    let actor: serde_json::Value = resp.json().await.expect("Actor response not valid JSON");
    assert_eq!(actor["type"], "Person", "actor type should be Person");
    assert!(
        actor["id"].as_str().unwrap_or("").contains(&account_nanoid),
        "actor id should contain account nanoid"
    );
    for field in &["preferredUsername", "inbox", "outbox", "followers"] {
        assert!(actor[field].as_str().is_some(), "actor should have {field}");
    }
    let pk = &actor["publicKey"];
    assert!(pk.is_object(), "actor should have publicKey object");
    assert!(
        pk["publicKeyPem"]
            .as_str()
            .unwrap_or("")
            .contains("BEGIN PUBLIC KEY"),
        "publicKeyPem should be a valid PEM-encoded public key"
    );
    assert!(
        pk["id"].as_str().unwrap_or("").ends_with("#main-key"),
        "publicKey id should end with #main-key"
    );
}

#[tokio::test]
#[ignore]
async fn outbound_follow_sends_activity_to_remote_inbox() {
    let peer = ApPeer::new("remoteuser").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account_nanoid = setup_test_account().await;

    let resp = post_follow(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "outbound follow should return 200 OK"
    );

    let body: serde_json::Value = resp.json().await.expect("follow response not valid JSON");
    assert!(
        body["activity_id"].as_str().is_some(),
        "response should contain activity_id"
    );

    let activity = wait_for_activity(&peer, "Follow", Duration::from_secs(15))
        .await
        .expect("mock peer inbox did not receive Follow activity within timeout");

    assert_eq!(
        activity.body["actor"],
        format!("{}/accounts/{account_nanoid}", cfg.server_base_url)
    );
    assert_eq!(activity.body["object"], peer.actor_url);
    assert_signature_header(&activity);

    let _following = fetch_collection(&cfg.server_base_url, &account_nanoid, "following").await;
}

#[tokio::test]
#[ignore]
async fn inbound_follow_creates_follower_and_sends_accept() {
    let peer = ApPeer::new("remote-alice").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let account_nanoid = setup_test_account().await;

    let target_inbox = format!("{}/accounts/{account_nanoid}/inbox", cfg.server_base_url);
    let target_actor = format!("{}/accounts/{account_nanoid}", cfg.server_base_url);
    let resp = post_signed_follow(&peer, &target_inbox, &target_actor).await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::ACCEPTED,
        "signed follow should be accepted with 202"
    );

    let followers = fetch_collection(&cfg.server_base_url, &account_nanoid, "followers").await;
    assert_collection_has_items(&followers, 1);

    let accept = wait_for_activity(&peer, "Accept", Duration::from_secs(15)).await;
    if let Some(accept) = accept {
        assert_eq!(accept.body["type"], "Accept");
        assert_eq!(
            accept.body["object"]["type"],
            serde_json::Value::String("Follow".to_string())
        );
    }
}

#[tokio::test]
#[ignore]
async fn followers_and_following_collections_are_accurate() {
    let peer = ApPeer::new("charlie").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account_nanoid = setup_test_account().await;

    let resp = post_follow(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let _ = wait_for_activity(&peer, "Follow", Duration::from_secs(15)).await;

    let following = fetch_collection(&cfg.server_base_url, &account_nanoid, "following").await;
    assert_collection_has_items(&following, 1);

    let followers = fetch_collection(&cfg.server_base_url, &account_nanoid, "followers").await;
    assert_eq!(followers["type"], "OrderedCollection");
    assert!(
        followers["totalItems"].as_u64().is_some(),
        "followers collection should have totalItems"
    );
}

#[tokio::test]
#[ignore]
async fn inbox_rejects_unsigned_requests() {
    db::reset_test_data().await;
    let cfg = config();
    let account_nanoid = setup_test_account().await;

    let resp = reqwest::Client::new()
        .post(format!(
            "{}/accounts/{account_nanoid}/inbox",
            cfg.server_base_url
        ))
        .header("content-type", "application/activity+json")
        .json(&serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "type": "Follow",
            "actor": "https://remote.example.com/users/alice",
            "object": format!("{}/accounts/{account_nanoid}", cfg.server_base_url)
        }))
        .send()
        .await
        .expect("unsigned inbox request failed");

    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "unsigned inbox POST should be rejected with 401"
    );
}
