//! ActivityPub Federation E2E Tests — Mock Peer Scenarios on the Profile federation model.
//!
//! Since the P4 cutover one Profile is one actor: `{PUBLIC_BASE_URL}/ap/profiles/{nanoid}`. These
//! scenarios run against a real server process and a mock peer that serves real HTTP (actor
//! document, WebFinger, public key, inbox), so key and actor fetching, the signatures in both
//! directions and the delivery worker are exercised end to end. Nothing in them uses the retired
//! Account based actor, the retired signing endpoint or a test cache for the peer's key.

#[allow(dead_code)]
mod support;

use std::time::Duration;

use support::account_helper::{
    assert_content_type, assert_signature_header, delete_block, delete_follow, e2e_http_client,
    fetch_profile_actor, follow_and_get_id, get_blocks, get_mutes, get_relations, post_block,
    post_follow, post_mute, post_signed_accept_direct, post_signed_block_direct,
    post_signed_follow_direct, post_signed_follow_with_id_direct, post_signed_undo_follow_direct,
    profile_inbox_urls, setup_test_account_details, start_server_with_peer, unique_username,
    verify_activity_signature, wait_for_relation_count, wait_for_relation_count_at_most,
};
use support::ap_peer::{generate_cavage_signature, wait_for_activity, ApPeer};
use support::auth;
use support::config::ap_e2e_config;
use support::db;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config() -> support::config::ApE2eConfig {
    ap_e2e_config()
}

fn public_domain(cfg: &support::config::ApE2eConfig) -> String {
    let parsed = url::Url::parse(&cfg.public_base_url).expect("valid public_base_url");
    let host = parsed
        .host_str()
        .expect("public_base_url must include a host");
    match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    }
}

#[tokio::test]
#[ignore]
async fn webfinger_resolves_the_profile_actor_by_username() {
    db::reset_test_data().await;
    let cfg = config();
    let account = setup_test_account_details().await;

    let resp = e2e_http_client()
        .get(format!("{}/.well-known/webfinger", cfg.server_base_url))
        .query(&[(
            "resource",
            &format!("acct:{}@{}", account.username, public_domain(&cfg)),
        )])
        .send()
        .await
        .expect("WebFinger request failed");

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_content_type(&resp, "application/jrd+json");
    let body: serde_json::Value = resp.json().await.expect("WebFinger JSON");
    assert!(
        body["subject"]
            .as_str()
            .expect("subject")
            .contains(&account.username),
        "subject should contain the Profile username with its . _ - characters"
    );
    let self_link = body["links"]
        .as_array()
        .expect("links")
        .iter()
        .find(|link| link["rel"] == "self")
        .expect("self link");
    assert_eq!(self_link["type"], "application/activity+json");
    assert!(
        self_link["href"]
            .as_str()
            .expect("href")
            .ends_with(&format!("/ap/profiles/{}", account.profile_id)),
        "the self link is the Profile actor, not an Account URL"
    );

    // The Account name is not an acct name any more.
    let by_account_name = e2e_http_client()
        .get(format!("{}/.well-known/webfinger", cfg.server_base_url))
        .query(&[(
            "resource",
            &format!("acct:{}@{}", account.name, public_domain(&cfg)),
        )])
        .send()
        .await
        .expect("WebFinger request failed");
    assert_eq!(by_account_name.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore]
async fn actor_document_publishes_the_profile_key() {
    db::reset_test_data().await;
    let cfg = config();
    let account = setup_test_account_details().await;

    let resp = e2e_http_client()
        .get(format!(
            "{}/ap/profiles/{}",
            cfg.server_base_url, account.profile_id
        ))
        .header(reqwest::header::ACCEPT, "application/activity+json")
        .send()
        .await
        .expect("Actor request failed");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_content_type(&resp, "application/activity+json");
    let actor: serde_json::Value = resp.json().await.expect("Actor JSON");

    assert_eq!(actor["type"], "Person");
    assert_eq!(actor["preferredUsername"], account.username.as_str());
    assert!(actor["id"]
        .as_str()
        .unwrap_or("")
        .ends_with(&format!("/ap/profiles/{}", account.profile_id)));
    for field in &["inbox", "outbox", "followers", "following"] {
        assert!(actor[field].as_str().is_some(), "actor should have {field}");
    }
    let (key_id, _encrypted, stored_public_pem) = db::active_profile_key(&account.profile_id).await;
    assert_eq!(
        actor["publicKey"]["publicKeyPem"].as_str(),
        Some(stored_public_pem.as_str()),
        "the actor document is where the Profile public key is obtained"
    );
    assert_eq!(actor["publicKey"]["id"].as_str(), Some(key_id.as_str()));
    assert!(key_id.ends_with("#main-key"));
}

#[tokio::test]
#[ignore]
async fn retired_account_federation_surface_answers_gone() {
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account = setup_test_account_details().await;
    let client = e2e_http_client();

    for suffix in ["", "/inbox", "/outbox", "/followers", "/following"] {
        let resp = client
            .get(format!(
                "{}/ap/accounts/{}{suffix}",
                cfg.server_base_url, account.id
            ))
            .header(reqwest::header::ACCEPT, "application/activity+json")
            .send()
            .await
            .expect("retired actor request failed");
        assert_eq!(resp.status(), reqwest::StatusCode::GONE, "{suffix}");
        assert!(
            resp.headers().get(reqwest::header::LOCATION).is_none(),
            "no redirect to the new actor ({suffix})"
        );
    }

    // No signing and no key export, not even for the owner with a valid token.
    let sign = client
        .post(format!(
            "{}/internal/v1/accounts/{}/sign",
            cfg.server_base_url, account.id
        ))
        .bearer_auth(&jwt)
        .json(&serde_json::json!({"method": "POST", "url": "https://example.com/inbox", "headers": {}, "body": ""}))
        .send()
        .await
        .expect("retired sign request failed");
    assert_eq!(sign.status(), reqwest::StatusCode::GONE);
    let public_key = client
        .get(format!(
            "{}/internal/v1/accounts/{}/public-key",
            cfg.server_base_url, account.id
        ))
        .bearer_auth(&jwt)
        .send()
        .await
        .expect("retired public-key request failed");
    assert_eq!(public_key.status(), reqwest::StatusCode::GONE);

    // The Account scoped relationship operations are gone; the Profile ones replace them.
    for (verb, tail) in [("POST", "follow"), ("GET", "followers"), ("POST", "block")] {
        let resp = client
            .request(
                verb.parse().unwrap(),
                format!(
                    "{}/api/v1/accounts/{}/{tail}",
                    cfg.server_base_url, account.id
                ),
            )
            .bearer_auth(&jwt)
            .json(&serde_json::json!({"target": "https://remote.example/users/x"}))
            .send()
            .await
            .expect("retired account relation request failed");
        assert_eq!(resp.status(), reqwest::StatusCode::GONE, "{verb} {tail}");
    }

    assert_eq!(
        db::legacy_federation_row_count().await,
        0,
        "nothing writes the legacy Account federation tables any more"
    );
}

#[tokio::test]
#[ignore]
async fn outbound_follow_delivers_a_follow_signed_with_the_profile_key() {
    let peer = ApPeer::new("remoteuser").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account = setup_test_account_details().await;

    let resp = post_follow(
        &jwt,
        &account.profile_id,
        &cfg.server_base_url,
        &peer.actor_url,
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::CREATED);
    let body: serde_json::Value = resp.json().await.expect("follow response JSON");
    assert_eq!(body["state"], "pending");
    assert_eq!(body["profile_id"], account.profile_id.as_str());
    assert_eq!(body["target"], peer.actor_url.as_str());

    let activity = wait_for_activity(&peer, "Follow", Duration::from_secs(15))
        .await
        .expect("mock peer inbox did not receive Follow within the timeout");
    let actor_url = format!(
        "{}/ap/profiles/{}",
        cfg.public_base_url.trim_end_matches('/'),
        account.profile_id
    );
    assert_eq!(activity.body["actor"], actor_url.as_str());
    assert_eq!(activity.body["object"], peer.actor_url.as_str());
    assert_signature_header(&activity);

    // The signature verifies against the public key the actor document publishes.
    let actor = fetch_profile_actor(&cfg.server_base_url, &account.profile_id).await;
    let key_id = actor["publicKey"]["id"]
        .as_str()
        .expect("key id")
        .to_string();
    assert_eq!(key_id, format!("{actor_url}#main-key"));
    let verified = verify_activity_signature(
        &activity,
        &key_id,
        actor["publicKey"]["publicKeyPem"].as_str().expect("pem"),
    )
    .await;
    assert_eq!(
        verified,
        kernel::interfaces::http_signing::SignatureVerificationResult::Valid { key_id }
    );

    let following =
        get_relations(&jwt, &account.profile_id, &cfg.server_base_url, "following").await;
    assert_eq!(following["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(following["items"][0]["state"], "pending");
}

#[tokio::test]
#[ignore]
async fn follow_by_acct_resolves_the_peer_over_webfinger() {
    let peer = ApPeer::new("acct-peer").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account = setup_test_account_details().await;
    let acct = format!("acct:acct-peer@127.0.0.1:{}", peer.address().port());

    let resp = post_follow(&jwt, &account.profile_id, &cfg.server_base_url, &acct).await;
    assert_eq!(resp.status(), reqwest::StatusCode::CREATED);
    let body: serde_json::Value = resp.json().await.expect("follow response JSON");
    assert_eq!(
        body["target"],
        peer.actor_url.as_str(),
        "the acct is stored as the canonical actor URL"
    );
    wait_for_activity(&peer, "Follow", Duration::from_secs(15))
        .await
        .expect("mock peer inbox did not receive Follow");
}

#[tokio::test]
#[ignore]
async fn unfollow_delivers_undo_and_a_failing_peer_does_not_undo_the_unfollow() {
    let peer = ApPeer::new("remote-unfollow").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account = setup_test_account_details().await;

    let follow_id = follow_and_get_id(
        &jwt,
        &account.profile_id,
        &cfg.server_base_url,
        &peer.actor_url,
    )
    .await;
    let follow = wait_for_activity(&peer, "Follow", Duration::from_secs(15))
        .await
        .expect("mock peer inbox did not receive Follow");
    let follow_activity_id = follow.body["id"].as_str().expect("Follow id").to_string();
    let (sign_inbox, send_inbox, target_actor) = profile_inbox_urls(&cfg, &account.profile_id);
    let accept = post_signed_accept_direct(
        &peer,
        &sign_inbox,
        &send_inbox,
        &follow_activity_id,
        &target_actor,
    )
    .await;
    assert_eq!(accept.status(), reqwest::StatusCode::ACCEPTED);
    let following =
        get_relations(&jwt, &account.profile_id, &cfg.server_base_url, "following").await;
    assert_eq!(following["items"][0]["state"], "accepted");
    peer.clear_inbox();

    let response = delete_follow(&jwt, &account.profile_id, &cfg.server_base_url, &follow_id).await;
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
    let undo = wait_for_activity(&peer, "Undo", Duration::from_secs(15))
        .await
        .expect("mock peer inbox did not receive Undo");
    assert_eq!(undo.body["actor"], follow.body["actor"]);
    assert_eq!(undo.body["object"]["type"], "Follow");
    assert_eq!(undo.body["object"]["id"], follow.body["id"]);
    assert_eq!(undo.body["object"]["object"], peer.actor_url.as_str());
    assert_signature_header(&undo);
    let following =
        get_relations(&jwt, &account.profile_id, &cfg.server_base_url, "following").await;
    assert_eq!(following["items"].as_array().map(Vec::len), Some(0));
    let missing = delete_follow(&jwt, &account.profile_id, &cfg.server_base_url, &follow_id).await;
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);

    // A peer whose inbox fails: the unfollow still succeeds (relationship state and delivery
    // state are separate), and the Undo stays queued for a retry.
    let failing = ApPeer::new("remote-unfollow-fail").await;
    let failing_follow_id = follow_and_get_id(
        &jwt,
        &account.profile_id,
        &cfg.server_base_url,
        &failing.actor_url,
    )
    .await;
    let failing_follow = wait_for_activity(&failing, "Follow", Duration::from_secs(15))
        .await
        .expect("failing peer did not receive Follow");
    let accept = post_signed_accept_direct(
        &failing,
        &sign_inbox,
        &send_inbox,
        failing_follow.body["id"].as_str().expect("Follow id"),
        &target_actor,
    )
    .await;
    assert_eq!(accept.status(), reqwest::StatusCode::ACCEPTED);
    failing.set_inbox_status(500);
    let response = delete_follow(
        &jwt,
        &account.profile_id,
        &cfg.server_base_url,
        &failing_follow_id,
    )
    .await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::NO_CONTENT,
        "a failing peer inbox must not undo the unfollow"
    );
    let mut state = None;
    for _ in 0..30 {
        state = db::delivery_job_state(&account.profile_id, "Undo").await;
        if matches!(&state, Some((s, attempts)) if s == "retrying" && *attempts >= 1) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        matches!(&state, Some((s, attempts)) if s == "retrying" && *attempts >= 1),
        "the failed Undo must be queued for a retry, got {state:?}"
    );
    let following =
        get_relations(&jwt, &account.profile_id, &cfg.server_base_url, "following").await;
    assert_eq!(following["items"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
#[ignore]
async fn inbound_follow_creates_a_follower_and_delivers_a_verifiable_accept() {
    let peer = ApPeer::new("remote-alice").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account = setup_test_account_details().await;
    let (sign_inbox, send_inbox, target_actor) = profile_inbox_urls(&cfg, &account.profile_id);

    let follow_id = format!("{}/activities/{}", peer.actor_url, uuid::Uuid::new_v4());
    let resp = post_signed_follow_with_id_direct(
        &peer,
        &sign_inbox,
        &send_inbox,
        &target_actor,
        &follow_id,
    )
    .await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::ACCEPTED,
        "a Follow signed by a peer whose key is fetched over HTTP is accepted"
    );

    assert!(
        wait_for_relation_count(
            &jwt,
            &account.profile_id,
            &cfg.server_base_url,
            "followers",
            1
        )
        .await,
        "the follower must be listed"
    );
    let followers =
        get_relations(&jwt, &account.profile_id, &cfg.server_base_url, "followers").await;
    assert_eq!(followers["items"][0]["state"], "accepted");
    assert_eq!(followers["items"][0]["follower"]["kind"], "remote");
    assert_eq!(
        followers["items"][0]["follower"]["actor_url"],
        peer.actor_url.as_str()
    );

    let accept = wait_for_activity(&peer, "Accept", Duration::from_secs(15))
        .await
        .expect("the Profile should deliver an Accept after a signed Follow");
    assert_eq!(accept.body["object"]["type"], "Follow");
    assert_eq!(accept.body["object"]["id"], follow_id.as_str());
    assert_signature_header(&accept);
    let actor = fetch_profile_actor(&cfg.server_base_url, &account.profile_id).await;
    let key_id = actor["publicKey"]["id"]
        .as_str()
        .expect("key id")
        .to_string();
    assert_eq!(
        verify_activity_signature(
            &accept,
            &key_id,
            actor["publicKey"]["publicKeyPem"].as_str().expect("pem"),
        )
        .await,
        kernel::interfaces::http_signing::SignatureVerificationResult::Valid { key_id }
    );

    // A duplicate delivery of the same Follow changes nothing and creates no second Accept.
    let again = post_signed_follow_with_id_direct(
        &peer,
        &sign_inbox,
        &send_inbox,
        &target_actor,
        &follow_id,
    )
    .await;
    assert_eq!(again.status(), reqwest::StatusCode::ACCEPTED);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        db::delivery_job_count(&account.profile_id, "Accept").await,
        1
    );
    let followers =
        get_relations(&jwt, &account.profile_id, &cfg.server_base_url, "followers").await;
    assert_eq!(followers["items"].as_array().map(Vec::len), Some(1));

    // The peer undoes its Follow: the follower disappears.
    let undo =
        post_signed_undo_follow_direct(&peer, &sign_inbox, &send_inbox, &target_actor, &follow_id)
            .await;
    assert_eq!(undo.status(), reqwest::StatusCode::ACCEPTED);
    assert!(
        wait_for_relation_count_at_most(
            &jwt,
            &account.profile_id,
            &cfg.server_base_url,
            "followers",
            0
        )
        .await,
        "Undo(Follow) must remove the follower"
    );
}

#[tokio::test]
#[ignore]
async fn inbox_rejects_unsigned_requests() {
    db::reset_test_data().await;
    let cfg = config();
    let account = setup_test_account_details().await;

    let resp = e2e_http_client()
        .post(format!(
            "{}/ap/profiles/{}/inbox",
            cfg.server_base_url, account.profile_id
        ))
        .header("content-type", "application/activity+json")
        .json(&serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "type": "Follow",
            "actor": "https://remote.example.com/users/alice",
            "object": format!("{}/ap/profiles/{}", cfg.server_base_url, account.profile_id)
        }))
        .send()
        .await
        .expect("unsigned inbox request failed");

    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "an unsigned inbox POST must be rejected with 401"
    );
}

/// Real HTTP key fetching must not be steerable at internal addresses: a key document on an
/// address that is not allowlisted is never requested, neither directly nor through a redirect.
#[tokio::test]
#[ignore]
async fn inbox_refuses_to_fetch_keys_from_internal_hosts_directly_or_by_redirect() {
    let peer = ApPeer::new("redirecting-peer").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let account = setup_test_account_details().await;
    let (sign_inbox, send_inbox, target_actor) = profile_inbox_urls(&cfg, &account.profile_id);

    // An "internal" host: loopback 127.0.0.2 is not in AP_TEST_ALLOWED_FETCH_HOSTS (127.0.0.1 is).
    let listener = std::net::TcpListener::bind("127.0.0.2:0")
        .expect("the SSRF scenario needs a second loopback address (127.0.0.2)");
    let internal = MockServer::builder().listener(listener).start().await;
    let internal_actor = format!("{}/users/internal", internal.uri());
    let evil_key = ApPeer::new("unused").await;
    Mock::given(method("GET"))
        .and(path("/users/internal"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": internal_actor,
            "type": "Person",
            "preferredUsername": "internal",
            "inbox": format!("{internal_actor}/inbox"),
            "publicKey": {
                "id": format!("{internal_actor}#main-key"),
                "owner": internal_actor,
                "publicKeyPem": evil_key.public_key_pem,
            },
        })))
        .mount(&internal)
        .await;

    let sign_as = |actor: String, key: Vec<u8>| {
        let sign_inbox = sign_inbox.clone();
        let target_actor = target_actor.clone();
        async move {
            let activity = serde_json::json!({
                "@context": "https://www.w3.org/ns/activitystreams",
                "type": "Follow",
                "actor": actor,
                "object": target_actor,
                "id": format!("{actor}/activities/{}", uuid::Uuid::new_v4()),
            });
            let body = serde_json::to_vec(&activity).unwrap();
            let headers = generate_cavage_signature(
                "POST",
                &sign_inbox,
                &body,
                &key,
                &format!("{actor}#main-key"),
            )
            .await;
            (body, headers)
        }
    };

    // Direct: the signer's key id points at the internal host.
    let (body, headers) = sign_as(internal_actor.clone(), evil_key.private_key_pem.clone()).await;
    let mut request = e2e_http_client().post(&send_inbox);
    for (name, value) in &headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let resp = request.body(body).send().await.expect("direct request");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert!(
        internal.received_requests().await.unwrap().is_empty(),
        "the internal host must not receive a request"
    );

    // Redirect: the allowed peer's key URL redirects to the internal host.
    Mock::given(method("GET"))
        .and(path("/users/redirected"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", internal_actor.as_str()))
        .mount(&peer.server)
        .await;
    let redirected_actor = format!("{}/users/redirected", peer.base_url);
    let (body, headers) = sign_as(redirected_actor, evil_key.private_key_pem.clone()).await;
    let mut request = e2e_http_client().post(&send_inbox);
    for (name, value) in &headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let resp = request.body(body).send().await.expect("redirect request");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert!(
        internal.received_requests().await.unwrap().is_empty(),
        "a redirect must not be followed to the internal host"
    );
    let followers = get_relations(
        &auth::get_jwt_for_test_user().await,
        &account.profile_id,
        &cfg.server_base_url,
        "followers",
    )
    .await;
    assert_eq!(followers["items"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
#[ignore]
async fn individual_block_detaches_relations_and_announces_an_activitypub_block() {
    let peer = ApPeer::new("remote-block").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account = setup_test_account_details().await;
    let (sign_inbox, send_inbox, target_actor) = profile_inbox_urls(&cfg, &account.profile_id);

    // Both directions: the peer follows the Profile (Accept delivered) and the Profile follows
    // the peer (Accept received).
    let peer_follow_id = format!("{}/activities/{}", peer.actor_url, uuid::Uuid::new_v4());
    let resp = post_signed_follow_with_id_direct(
        &peer,
        &sign_inbox,
        &send_inbox,
        &target_actor,
        &peer_follow_id,
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    wait_for_activity(&peer, "Accept", Duration::from_secs(15))
        .await
        .expect("Accept for the peer's Follow");
    follow_and_get_id(
        &jwt,
        &account.profile_id,
        &cfg.server_base_url,
        &peer.actor_url,
    )
    .await;
    let follow = wait_for_activity(&peer, "Follow", Duration::from_secs(15))
        .await
        .expect("Follow of the Profile");
    let accept = post_signed_accept_direct(
        &peer,
        &sign_inbox,
        &send_inbox,
        follow.body["id"].as_str().expect("Follow id"),
        &target_actor,
    )
    .await;
    assert_eq!(accept.status(), reqwest::StatusCode::ACCEPTED);
    peer.clear_inbox();

    let response = post_block(
        &jwt,
        &account.profile_id,
        &cfg.server_base_url,
        &peer.actor_url,
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let block: serde_json::Value = response.json().await.expect("block response JSON");
    let block_id = block["id"].as_str().expect("block id").to_string();

    // Both relations are detached at once and the peer is told only what is needed: the Profile
    // stops following (Undo) and refuses the peer's Follow (Reject); an individual block also
    // announces an ActivityPub Block whose object is the peer actor (a domain block would not).
    let undo = wait_for_activity(&peer, "Undo", Duration::from_secs(15))
        .await
        .expect("Undo(Follow) for the Profile's own Follow");
    assert_eq!(undo.body["object"]["id"], follow.body["id"]);
    let reject = wait_for_activity(&peer, "Reject", Duration::from_secs(15))
        .await
        .expect("Reject(Follow) for the peer's Follow");
    assert_eq!(reject.body["object"]["id"], peer_follow_id.as_str());
    let block_activity = wait_for_activity(&peer, "Block", Duration::from_secs(15))
        .await
        .expect("ActivityPub Block for the individually blocked actor");
    assert_eq!(block_activity.body["actor"], target_actor.as_str());
    assert_eq!(block_activity.body["object"], peer.actor_url.as_str());
    assert_signature_header(&block_activity);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        peer.received_activities()
            .iter()
            .filter(|activity| activity.body["type"] == "Block")
            .count(),
        1,
        "exactly one Block is delivered"
    );
    for list in ["following", "followers"] {
        let relations = get_relations(&jwt, &account.profile_id, &cfg.server_base_url, list).await;
        assert_eq!(
            relations["items"].as_array().map(Vec::len),
            Some(0),
            "{list}"
        );
    }
    let blocks = get_blocks(&jwt, &account.profile_id, &cfg.server_base_url).await;
    assert_eq!(blocks["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(blocks["items"][0]["target"], peer.actor_url.as_str());

    // While blocked the peer cannot follow again; the refusal is answered with a Reject. Its
    // delivery is awaited, so that it cannot land in the "removing the block delivers nothing"
    // window below.
    peer.clear_inbox();
    let refused_follow_id = format!("{}/activities/{}", peer.actor_url, uuid::Uuid::new_v4());
    let refused = post_signed_follow_with_id_direct(
        &peer,
        &sign_inbox,
        &send_inbox,
        &target_actor,
        &refused_follow_id,
    )
    .await;
    assert_eq!(refused.status(), reqwest::StatusCode::ACCEPTED);
    let refusal = wait_for_activity(&peer, "Reject", Duration::from_secs(15))
        .await
        .expect("Reject(Follow) for the refused Follow");
    assert_eq!(refusal.body["object"]["id"], refused_follow_id.as_str());
    let followers =
        get_relations(&jwt, &account.profile_id, &cfg.server_base_url, "followers").await;
    assert_eq!(followers["items"].as_array().map(Vec::len), Some(0));

    // Removing the block restores nothing; the only thing delivered is the Undo of the Block.
    peer.clear_inbox();
    let response = delete_block(&jwt, &account.profile_id, &cfg.server_base_url, &block_id).await;
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
    let undo_block = wait_for_activity(&peer, "Undo", Duration::from_secs(15))
        .await
        .expect("Undo(Block) for the removed block");
    assert_eq!(undo_block.body["actor"], target_actor.as_str());
    assert_eq!(undo_block.body["object"]["type"], "Block");
    assert_eq!(undo_block.body["object"]["id"], block_activity.body["id"]);
    assert_eq!(undo_block.body["object"]["object"], peer.actor_url.as_str());
    assert_signature_header(&undo_block);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        peer.received_activities().len(),
        1,
        "nothing but the Undo(Block) is delivered"
    );
    for list in ["following", "followers"] {
        let relations = get_relations(&jwt, &account.profile_id, &cfg.server_base_url, list).await;
        assert_eq!(
            relations["items"].as_array().map(Vec::len),
            Some(0),
            "{list}"
        );
    }
    let missing = delete_block(&jwt, &account.profile_id, &cfg.server_base_url, &block_id).await;
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore]
async fn inbound_activitypub_block_is_acknowledged_and_changes_nothing() {
    let peer = ApPeer::new("remote-blocker").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account = setup_test_account_details().await;
    let (sign_inbox, send_inbox, target_actor) = profile_inbox_urls(&cfg, &account.profile_id);

    let follow = post_signed_follow_direct(&peer, &sign_inbox, &send_inbox, &target_actor).await;
    assert_eq!(follow.status(), reqwest::StatusCode::ACCEPTED);
    assert!(
        wait_for_relation_count(
            &jwt,
            &account.profile_id,
            &cfg.server_base_url,
            "followers",
            1
        )
        .await
    );

    let block = post_signed_block_direct(&peer, &sign_inbox, &send_inbox, &target_actor).await;
    assert_eq!(block.status(), reqwest::StatusCode::ACCEPTED);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let followers =
        get_relations(&jwt, &account.profile_id, &cfg.server_base_url, "followers").await;
    assert_eq!(
        followers["items"].as_array().map(Vec::len),
        Some(1),
        "a received Block is not interpreted (domain policy belongs to D8G/D8R)"
    );
}

#[tokio::test]
#[ignore]
async fn mute_is_idempotent_per_target_and_changes_no_relation() {
    let peer = ApPeer::new("remote-mute").await;
    let _server = start_server_with_peer(&peer).await;
    db::reset_test_data().await;
    let cfg = config();
    let jwt = auth::get_jwt_for_test_user().await;
    let account = setup_test_account_details().await;

    let first = post_mute(
        &jwt,
        &account.profile_id,
        &cfg.server_base_url,
        &peer.actor_url,
    )
    .await;
    assert_eq!(first.status(), reqwest::StatusCode::CREATED);
    let created: serde_json::Value = first.json().await.expect("mute response JSON");
    let second = post_mute(
        &jwt,
        &account.profile_id,
        &cfg.server_base_url,
        &peer.actor_url,
    )
    .await;
    assert_eq!(second.status(), reqwest::StatusCode::CONFLICT);

    let mutes = get_mutes(&jwt, &account.profile_id, &cfg.server_base_url).await;
    assert_eq!(mutes["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(mutes["items"][0]["id"], created["id"]);
    assert_eq!(
        db::delivery_job_count(&account.profile_id, "Follow").await,
        0
    );
    assert!(
        peer.received_activities().is_empty(),
        "a mute delivers nothing"
    );

    let removed = e2e_http_client()
        .delete(format!(
            "{}/api/v1/profiles/{}/mutes/{}",
            cfg.server_base_url,
            account.profile_id,
            created["id"].as_str().expect("mute id")
        ))
        .bearer_auth(&jwt)
        .send()
        .await
        .expect("unmute request failed");
    assert_eq!(removed.status(), reqwest::StatusCode::NO_CONTENT);
    let mutes = get_mutes(&jwt, &account.profile_id, &cfg.server_base_url).await;
    assert_eq!(mutes["items"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
#[ignore]
async fn usernames_with_dot_underscore_and_hyphen_are_actors() {
    db::reset_test_data().await;
    let cfg = config();
    let username = unique_username("a-b.c_d");
    let account = support::account_helper::setup_test_account_with_username(&username).await;
    let actor = fetch_profile_actor(&cfg.server_base_url, &account.profile_id).await;
    assert_eq!(actor["preferredUsername"], username.as_str());
    let resp = e2e_http_client()
        .get(format!("{}/.well-known/webfinger", cfg.server_base_url))
        .query(&[(
            "resource",
            &format!("acct:{username}@{}", public_domain(&cfg)),
        )])
        .send()
        .await
        .expect("WebFinger request failed");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
}
