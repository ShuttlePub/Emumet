//! ActivityPub Federation E2E Tests — Mock Peer Scenarios (S1–S6)

#[allow(dead_code)]
mod support;

use std::time::Duration;

use support::account_helper::{
    assert_collection_has_items, assert_content_type, assert_signature_header, e2e_http_client,
    fetch_collection, get_blocks, get_mutes, post_block, post_follow, post_mute,
    post_signed_accept_direct, post_signed_block_direct, post_signed_follow_direct,
    post_signed_undo_block_direct, post_unblock, post_unfollow, setup_test_account_details,
    start_server_with_peer,
};
use support::ap_peer::{wait_for_activity, ApPeer};
use support::auth;
use support::config::ap_e2e_config;
use support::db;

fn config() -> support::config::ApE2eConfig {
    ap_e2e_config()
}

#[tokio::test]
#[ignore]
async fn webfinger_resolves_account() {
    db::reset_test_data().await;
    let cfg = config();
    let account = setup_test_account_details().await;

    let public_domain = url::Url::parse(&cfg.public_base_url)
        .expect("valid public_base_url")
        .host_str()
        .map(|h| {
            let port = url::Url::parse(&cfg.public_base_url)
                .ok()
                .and_then(|u| u.port());
            match port {
                Some(p) => format!("{h}:{p}"),
                None => h.to_string(),
            }
        })
        .expect("public_base_url must include a host for WebFinger resource domain");

    let resp = e2e_http_client()
        .get(format!("{}/.well-known/webfinger", cfg.server_base_url))
        .query(&[(
            "resource",
            &format!("acct:{}@{public_domain}", account.name),
        )])
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
        subject.contains(&account.name),
        "subject should contain account name: {subject}"
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
        href.contains(&account.id),
        "self link href should contain account ID: {href}"
    );
}

#[tokio::test]
#[ignore]
async fn actor_document_is_valid_activitypub() {
    db::reset_test_data().await;
    let cfg = config();
    let account_nanoid = setup_test_account_details().await.id;

    let resp = e2e_http_client()
        .get(format!(
            "{}/ap/accounts/{account_nanoid}",
            cfg.server_base_url
        ))
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
    let account_nanoid = setup_test_account_details().await.id;

    let resp = post_follow(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "outbound follow should return 200 OK"
    );

    let body: serde_json::Value = resp.json().await.expect("follow response not valid JSON");
    assert!(
        body["activityId"].as_str().is_some(),
        "response should contain activityId"
    );

    let activity = wait_for_activity(&peer, "Follow", Duration::from_secs(15))
        .await
        .expect("mock peer inbox did not receive Follow activity within timeout");

    assert_eq!(
        activity.body["actor"],
        format!("{}/ap/accounts/{account_nanoid}", cfg.public_base_url)
    );
    assert_eq!(activity.body["object"], peer.actor_url);
    assert_signature_header(&activity);

    let _following = fetch_collection(&cfg.server_base_url, &account_nanoid, "following").await;
}

#[tokio::test]
#[ignore]
async fn outbound_unfollow_sends_undo_to_remote_inbox() {
    let peer = ApPeer::new("remote-unfollow").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account_nanoid = setup_test_account_details().await.id;

    let follow_response =
        post_follow(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(follow_response.status(), reqwest::StatusCode::OK);
    let follow = wait_for_activity(&peer, "Follow", Duration::from_secs(15))
        .await
        .expect("mock peer inbox did not receive Follow activity");

    let follow_activity_id = follow.body["id"]
        .as_str()
        .expect("Follow activity missing id");
    let sign_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.public_base_url);
    let send_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.server_base_url);
    let target_actor = format!("{}/ap/accounts/{account_nanoid}", cfg.public_base_url);
    let accept = post_signed_accept_direct(
        &peer,
        &sign_inbox,
        &send_inbox,
        follow_activity_id,
        &target_actor,
    )
    .await;
    assert_eq!(accept.status(), reqwest::StatusCode::ACCEPTED);

    peer.set_inbox_status(500);
    let failed = post_unfollow(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(failed.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    peer.set_inbox_status(202);
    peer.clear_inbox();

    let response =
        post_unfollow(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
    let undo = wait_for_activity(&peer, "Undo", Duration::from_secs(15))
        .await
        .expect("mock peer inbox did not receive Undo activity");

    assert_eq!(undo.body["actor"], follow.body["actor"]);
    assert_eq!(undo.body["object"]["type"], "Follow");
    assert_eq!(undo.body["object"]["id"], follow.body["id"]);
    assert_eq!(undo.body["object"]["object"], peer.actor_url);
    assert_signature_header(&undo);

    let following = fetch_collection(&cfg.server_base_url, &account_nanoid, "following").await;
    assert_eq!(following["totalItems"], 0);
    let missing = post_unfollow(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore]
async fn inbound_follow_creates_follower_and_sends_accept() {
    let peer = ApPeer::new("remote-alice").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let account_nanoid = setup_test_account_details().await.id;

    let sign_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.public_base_url);
    let send_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.server_base_url);
    let target_actor = format!("{}/ap/accounts/{account_nanoid}", cfg.public_base_url);
    let resp = post_signed_follow_direct(&peer, &sign_inbox, &send_inbox, &target_actor).await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::ACCEPTED,
        "signed follow should be accepted with 202"
    );

    let followers = fetch_collection(&cfg.server_base_url, &account_nanoid, "followers").await;
    assert_collection_has_items(&followers, 1);

    let accept = wait_for_activity(&peer, "Accept", Duration::from_secs(15))
        .await
        .expect("Emumet should send Accept activity within timeout after receiving signed Follow");
    assert_eq!(accept.body["type"], "Accept");
    assert_eq!(
        accept.body["object"]["type"],
        serde_json::Value::String("Follow".to_string())
    );
}

#[tokio::test]
#[ignore]
async fn followers_and_following_collections_are_accurate() {
    let peer = ApPeer::new("charlie").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account_nanoid = setup_test_account_details().await.id;

    let resp = post_follow(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    // Wait for the mock peer to receive the Follow activity
    let follow_activity = wait_for_activity(&peer, "Follow", Duration::from_secs(15))
        .await
        .expect("mock peer did not receive Follow activity");

    // Send a signed Accept back to Emumet to approve the follow
    let follow_activity_id = follow_activity.body["id"]
        .as_str()
        .expect("Follow activity missing id");
    let sign_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.public_base_url);
    let send_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.server_base_url);
    let target_actor = format!("{}/ap/accounts/{account_nanoid}", cfg.public_base_url);
    let accept_resp = post_signed_accept_direct(
        &peer,
        &sign_inbox,
        &send_inbox,
        follow_activity_id,
        &target_actor,
    )
    .await;
    assert_eq!(
        accept_resp.status(),
        reqwest::StatusCode::ACCEPTED,
        "signed Accept should be accepted with 202"
    );

    // Now the following collection should show the approved follow
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
    let account_nanoid = setup_test_account_details().await.id;

    let resp = e2e_http_client()
        .post(format!(
            "{}/ap/accounts/{account_nanoid}/inbox",
            cfg.server_base_url
        ))
        .header("content-type", "application/activity+json")
        .json(&serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "type": "Follow",
            "actor": "https://remote.example.com/users/alice",
            "object": format!("{}/ap/accounts/{account_nanoid}", cfg.server_base_url)
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

#[tokio::test]
#[ignore]
async fn outbound_block_sends_block_to_remote_inbox() {
    let peer = ApPeer::new("remote-block").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account_nanoid = setup_test_account_details().await.id;

    // Success path: the block is committed and the Block activity is
    // delivered post-commit, marking the outbox row as delivered.
    let response = post_block(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "block should return 200 OK"
    );

    let activity = wait_for_activity(&peer, "Block", Duration::from_secs(15))
        .await
        .expect("mock peer inbox did not receive Block activity within timeout");

    assert_eq!(
        activity.body["actor"],
        format!("{}/ap/accounts/{account_nanoid}", cfg.public_base_url)
    );
    assert_eq!(activity.body["object"], peer.actor_url);
    assert_signature_header(&activity);

    let blocks = get_blocks(&jwt, &account_nanoid, &cfg.server_base_url).await;
    let items = blocks["items"]
        .as_array()
        .expect("blocks should have items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["target"], peer.actor_url);
    assert_eq!(items[0]["targetType"], "remote");

    assert_eq!(
        db::outbox_delivery_state(&account_nanoid, "Block").await,
        Some((true, false, false)),
        "successful delivery should mark the outbox row delivered"
    );

    // Failure path (transactional outbox semantics): DB writes and the outbox
    // row commit first and delivery happens post-commit, so a failing peer
    // inbox no longer fails the operation — the API returns 200, the block
    // persists, and the outbox row records the failed attempt (retryable).
    let failing_peer = ApPeer::new("remote-block-fail").await;
    failing_peer.set_inbox_status(500);
    let failed = post_block(
        &jwt,
        &account_nanoid,
        &cfg.server_base_url,
        &failing_peer.actor_url,
    )
    .await;
    assert_eq!(
        failed.status(),
        reqwest::StatusCode::OK,
        "block delivery failure must not fail the operation (post-commit delivery)"
    );

    let blocks = get_blocks(&jwt, &account_nanoid, &cfg.server_base_url).await;
    assert_eq!(
        blocks["items"].as_array().map(Vec::len),
        Some(2),
        "block must persist even when delivery fails"
    );

    assert_eq!(
        db::outbox_delivery_state(&account_nanoid, "Block").await,
        Some((false, true, true)),
        "failed delivery should leave a retryable outbox row (attempted, not delivered, error set)"
    );
}

#[tokio::test]
#[ignore]
async fn outbound_unblock_sends_undo_block_to_remote_inbox() {
    let peer = ApPeer::new("remote-unblock").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account_nanoid = setup_test_account_details().await.id;

    let block_response =
        post_block(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(block_response.status(), reqwest::StatusCode::OK);
    let block = wait_for_activity(&peer, "Block", Duration::from_secs(15))
        .await
        .expect("mock peer inbox did not receive Block activity");
    peer.clear_inbox();

    let response = post_unblock(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
    let undo = wait_for_activity(&peer, "Undo", Duration::from_secs(15))
        .await
        .expect("mock peer inbox did not receive Undo activity");

    assert_eq!(undo.body["actor"], block.body["actor"]);
    assert_eq!(undo.body["object"]["type"], "Block");
    assert_eq!(undo.body["object"]["id"], block.body["id"]);
    assert_eq!(undo.body["object"]["object"], peer.actor_url);
    assert_signature_header(&undo);

    let blocks = get_blocks(&jwt, &account_nanoid, &cfg.server_base_url).await;
    assert_eq!(blocks["items"].as_array().map(Vec::len), Some(0));
    let missing = post_unblock(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);

    // Failure path (transactional outbox semantics): a failing peer inbox does
    // not fail the unblock — the block deletion is committed with the outbox
    // row first, so the API returns 204, the block stays deleted, and the
    // Undo outbox row records the failed attempt (retryable).
    let reblock = post_block(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(reblock.status(), reqwest::StatusCode::OK);
    wait_for_activity(&peer, "Block", Duration::from_secs(15))
        .await
        .expect("mock peer inbox did not receive Block activity");
    peer.clear_inbox();
    peer.set_inbox_status(500);

    let failed = post_unblock(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(
        failed.status(),
        reqwest::StatusCode::NO_CONTENT,
        "unblock delivery failure must not fail the operation (post-commit delivery)"
    );

    let blocks = get_blocks(&jwt, &account_nanoid, &cfg.server_base_url).await;
    assert_eq!(
        blocks["items"].as_array().map(Vec::len),
        Some(0),
        "block must stay deleted even when Undo delivery fails"
    );

    assert_eq!(
        db::outbox_delivery_state(&account_nanoid, "Undo").await,
        Some((false, true, true)),
        "failed Undo delivery should leave a retryable outbox row (attempted, not delivered, error set)"
    );
}

#[tokio::test]
#[ignore]
async fn inbound_block_creates_block_and_removes_follows() {
    let peer = ApPeer::new("remote-blocker").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let account_nanoid = setup_test_account_details().await.id;

    let sign_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.public_base_url);
    let send_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.server_base_url);
    let target_actor = format!("{}/ap/accounts/{account_nanoid}", cfg.public_base_url);
    let follow_resp =
        post_signed_follow_direct(&peer, &sign_inbox, &send_inbox, &target_actor).await;
    assert_eq!(follow_resp.status(), reqwest::StatusCode::ACCEPTED);
    let followers = fetch_collection(&cfg.server_base_url, &account_nanoid, "followers").await;
    assert_collection_has_items(&followers, 1);

    let block_resp = post_signed_block_direct(&peer, &sign_inbox, &send_inbox, &target_actor).await;
    assert_eq!(
        block_resp.status(),
        reqwest::StatusCode::ACCEPTED,
        "signed Block should be accepted with 202"
    );

    assert_eq!(
        db::count_remote_blocks_against_local_account(&account_nanoid).await,
        1,
        "inbound Block should create a remote-to-local block row"
    );

    let followers = fetch_collection(&cfg.server_base_url, &account_nanoid, "followers").await;
    assert_eq!(
        followers["totalItems"], 0,
        "inbound Block should remove the follow relationship"
    );
}

#[tokio::test]
#[ignore]
async fn inbound_undo_block_removes_block() {
    let peer = ApPeer::new("remote-unblocker").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let account_nanoid = setup_test_account_details().await.id;

    let sign_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.public_base_url);
    let send_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.server_base_url);
    let target_actor = format!("{}/ap/accounts/{account_nanoid}", cfg.public_base_url);
    let block_resp = post_signed_block_direct(&peer, &sign_inbox, &send_inbox, &target_actor).await;
    assert_eq!(block_resp.status(), reqwest::StatusCode::ACCEPTED);
    assert_eq!(
        db::count_remote_blocks_against_local_account(&account_nanoid).await,
        1
    );

    let undo_resp =
        post_signed_undo_block_direct(&peer, &sign_inbox, &send_inbox, &target_actor).await;
    assert_eq!(
        undo_resp.status(),
        reqwest::StatusCode::ACCEPTED,
        "signed Undo(Block) should be accepted with 202"
    );

    assert_eq!(
        db::count_remote_blocks_against_local_account(&account_nanoid).await,
        0,
        "inbound Undo(Block) should remove the remote-to-local block row"
    );
}

#[tokio::test]
#[ignore]
async fn duplicate_inbound_follow_sends_single_accept() {
    let peer = ApPeer::new("remote-dup-follow").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let account_nanoid = setup_test_account_details().await.id;

    let sign_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.public_base_url);
    let send_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.server_base_url);
    let target_actor = format!("{}/ap/accounts/{account_nanoid}", cfg.public_base_url);

    let first = post_signed_follow_direct(&peer, &sign_inbox, &send_inbox, &target_actor).await;
    assert_eq!(
        first.status(),
        reqwest::StatusCode::ACCEPTED,
        "signed follow should be accepted with 202"
    );
    wait_for_activity(&peer, "Accept", Duration::from_secs(15))
        .await
        .expect("Emumet should send Accept activity for the first Follow");

    // Inbound Follow is idempotent on the follow relationship: a duplicate
    // Follow from the same actor to the same target is accepted but creates
    // neither a second follow row nor a second Accept delivery.
    let duplicate = post_signed_follow_direct(&peer, &sign_inbox, &send_inbox, &target_actor).await;
    assert_eq!(
        duplicate.status(),
        reqwest::StatusCode::ACCEPTED,
        "duplicate signed follow should still be accepted with 202"
    );

    let accepts = peer
        .received_activities()
        .iter()
        .filter(|a| a.body["type"] == "Accept")
        .count();
    assert_eq!(
        accepts, 1,
        "duplicate Follow must not trigger a second Accept"
    );

    let followers = fetch_collection(&cfg.server_base_url, &account_nanoid, "followers").await;
    assert_eq!(
        followers["totalItems"], 1,
        "duplicate Follow must not create a second follower"
    );
}

#[tokio::test]
#[ignore]
async fn mute_twice_returns_ok_with_single_mute() {
    let peer = ApPeer::new("remote-mute").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account_nanoid = setup_test_account_details().await.id;

    let first = post_mute(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(
        first.status(),
        reqwest::StatusCode::OK,
        "first mute should return 200 OK"
    );

    // Mute is idempotent: muting the same target again returns 200 instead of
    // the old 422, and still results in a single mute relationship.
    let second = post_mute(&jwt, &account_nanoid, &cfg.server_base_url, &peer.actor_url).await;
    assert_eq!(
        second.status(),
        reqwest::StatusCode::OK,
        "duplicate mute should return 200 OK"
    );

    let mutes = get_mutes(&jwt, &account_nanoid, &cfg.server_base_url).await;
    let items = mutes["items"].as_array().expect("mutes should have items");
    assert_eq!(
        items.len(),
        1,
        "duplicate mute must not create a second mute row"
    );
    assert_eq!(items[0]["target"], peer.actor_url);
}

#[tokio::test]
#[ignore]
async fn undo_block_twice_returns_ok() {
    let peer = ApPeer::new("remote-dup-undo-block").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let account_nanoid = setup_test_account_details().await.id;

    let sign_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.public_base_url);
    let send_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.server_base_url);
    let target_actor = format!("{}/ap/accounts/{account_nanoid}", cfg.public_base_url);

    let block_resp = post_signed_block_direct(&peer, &sign_inbox, &send_inbox, &target_actor).await;
    assert_eq!(block_resp.status(), reqwest::StatusCode::ACCEPTED);
    assert_eq!(
        db::count_remote_blocks_against_local_account(&account_nanoid).await,
        1
    );

    let first = post_signed_undo_block_direct(&peer, &sign_inbox, &send_inbox, &target_actor).await;
    assert_eq!(
        first.status(),
        reqwest::StatusCode::ACCEPTED,
        "signed Undo(Block) should be accepted with 202"
    );
    assert_eq!(
        db::count_remote_blocks_against_local_account(&account_nanoid).await,
        0
    );

    // Inbox Undo(Block) is idempotent: a duplicate Undo is accepted with 202
    // and leaves the already-removed block relationship untouched. (The REST
    // unblock of a missing block still returns 404 — only the inbox flow is
    // idempotent.)
    let duplicate =
        post_signed_undo_block_direct(&peer, &sign_inbox, &send_inbox, &target_actor).await;
    assert_eq!(
        duplicate.status(),
        reqwest::StatusCode::ACCEPTED,
        "duplicate inbound Undo(Block) should be accepted with 202"
    );
    assert_eq!(
        db::count_remote_blocks_against_local_account(&account_nanoid).await,
        0
    );
}

const PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72, 0xb6, 0x0d,
    0x24, 0x00, 0x00, 0x00, 0x11, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x84, 0x19, 0x60, 0x0c, 0x00, 0x47, 0xca, 0x07, 0xf9, 0x67, 0x59, 0x6e, 0xb7, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

#[tokio::test]
#[ignore]
async fn icon_change_delivers_signed_update_person_to_follower() {
    // Given: a local account followed by an approved remote peer.
    let peer = ApPeer::new("remote-upd").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let account_nanoid = setup_test_account_details().await.id;
    let jwt = auth::get_jwt_for_test_user().await;
    let client = e2e_http_client();

    let sign_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.public_base_url);
    let send_inbox = format!("{}/ap/accounts/{account_nanoid}/inbox", cfg.server_base_url);
    let target_actor = format!("{}/ap/accounts/{account_nanoid}", cfg.public_base_url);
    let follow = post_signed_follow_direct(&peer, &sign_inbox, &send_inbox, &target_actor).await;
    assert_eq!(follow.status(), reqwest::StatusCode::ACCEPTED);
    wait_for_activity(&peer, "Accept", Duration::from_secs(15))
        .await
        .expect("Emumet should accept the remote follow");

    // When: only the display name changes (no icon/banner change).
    let name_patch = client
        .patch(format!(
            "{}/api/v1/accounts/{account_nanoid}",
            cfg.server_base_url
        ))
        .bearer_auth(&jwt)
        .json(&serde_json::json!({"display_name": "No Delivery Expected"}))
        .send()
        .await
        .expect("display-name patch failed");

    // Then: the patch succeeds but no Update(Person) is delivered.
    assert_eq!(name_patch.status(), reqwest::StatusCode::OK);
    assert!(
        wait_for_activity(&peer, "Update", Duration::from_secs(5))
            .await
            .is_none(),
        "display-name-only change must not deliver Update(Person)"
    );

    // When: an uploaded image is set as the account icon.
    let file_part = reqwest::multipart::Part::bytes(PIXEL_PNG.to_vec())
        .file_name("icon.png")
        .mime_str("image/png")
        .expect("invalid MIME type");
    let uploaded = client
        .post(format!("{}/api/v1/images", cfg.server_base_url))
        .bearer_auth(&jwt)
        .multipart(reqwest::multipart::Form::new().part("file", file_part))
        .send()
        .await
        .expect("image upload failed");
    assert_eq!(uploaded.status(), reqwest::StatusCode::CREATED);
    let uploaded: serde_json::Value = uploaded.json().await.expect("invalid upload response");
    let icon_url = uploaded["url"].as_str().expect("missing url").to_string();

    let icon_patch = client
        .patch(format!(
            "{}/api/v1/accounts/{account_nanoid}",
            cfg.server_base_url
        ))
        .bearer_auth(&jwt)
        .json(&serde_json::json!({"icon_url": icon_url}))
        .send()
        .await
        .expect("icon patch failed");
    assert_eq!(icon_patch.status(), reqwest::StatusCode::OK);

    // Then: the follower receives a signed Update wrapping the Person with the new icon.
    let update = wait_for_activity(&peer, "Update", Duration::from_secs(15))
        .await
        .expect("Emumet should deliver Update(Person) to follower inbox after icon change");
    assert_signature_header(&update);
    assert_eq!(update.body["object"]["type"], "Person");
    assert_eq!(update.body["object"]["icon"]["url"], icon_url);
}
