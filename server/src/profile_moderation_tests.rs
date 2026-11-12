//! A suspended or banned Account's Profile through the production root router and a real
//! database: no actor, no WebFinger, no inbox, no new Follow in either direction and no delivery
//! while the restriction lasts; the same identity and key, and the queued delivery, once it ends.

use crate::api::profile_delivery_worker::spawn_profile_delivery_worker;
use crate::route::profile_graph_test_support::{GraphWorld, LocalProfile, RemotePeer, User};
use application::projection::{ProjectAccountBatch, ProjectProfileBatch};
use application::service::account::{
    BanAccountUseCase, SuspendAccountUseCase, UnbanAccountUseCase, UnsuspendAccountUseCase,
};
use application::service::profile_relationship::ProfileRelationshipUseCase;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use driver::http_signing::HttpSignatureVerifierImpl;
use http_body_util::BodyExt;
use kernel::interfaces::http_signing::{
    HttpSignatureVerificationInput, HttpSignatureVerifier, SignatureVerificationResult,
};
use kernel::prelude::entity::ProfileDeliveryPolicy;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Clone, Copy)]
enum Restriction {
    Suspend,
    Ban,
}

async fn call(
    world: &GraphWorld,
    http_method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    send(world, world.root_router(), http_method, uri, token, body).await
}

/// Same, on a root router whose outgoing fetches may reach the loopback fake peer.
async fn call_loopback(
    world: &GraphWorld,
    http_method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    send(
        world,
        world.loopback_root_router(),
        http_method,
        uri,
        token,
        body,
    )
    .await
}

async fn send(
    world: &GraphWorld,
    router: axum::Router,
    http_method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(http_method)
        .uri(uri)
        .header("host", world.host_header.as_str())
        .header("accept", "application/activity+json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn count(world: &GraphWorld, sql: &str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(world.pool())
        .await
        .unwrap()
}

async fn project(world: &GraphWorld) {
    for _ in 0..4 {
        world.module.project_batch().await.unwrap();
        world.module.project_profile_batch().await.unwrap();
    }
}

async fn webfinger(world: &GraphWorld, username: &str) -> StatusCode {
    call(
        world,
        "GET",
        &format!(
            "/.well-known/webfinger?resource=acct:{username}@{}",
            world.host_header
        ),
        None,
        None,
    )
    .await
    .0
}

async fn run_worker(world: &GraphWorld, for_how_long: Duration) {
    let module = world.module.clone().with_loopback_profile_delivery();
    let (shutdown, handle) = spawn_profile_delivery_worker(
        Arc::new(module),
        ProfileDeliveryPolicy::default(),
        Duration::from_millis(50),
    );
    tokio::time::sleep(for_how_long).await;
    shutdown.trigger();
    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("the worker stops")
        .unwrap();
}

async fn posts_to(peer: &MockServer) -> Vec<wiremock::Request> {
    peer.received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.method.as_str() == "POST")
        .collect()
}

async fn restrict(world: &GraphWorld, moderator: &User, account_nanoid: &str, kind: Restriction) {
    match kind {
        Restriction::Suspend => world
            .module
            .suspend_account(
                &moderator.auth_account_id,
                account_nanoid.to_string(),
                "moderation test".to_string(),
                None,
            )
            .await
            .unwrap(),
        Restriction::Ban => world
            .module
            .ban_account(
                &moderator.auth_account_id,
                account_nanoid.to_string(),
                "moderation test".to_string(),
            )
            .await
            .unwrap(),
    }
    project(world).await;
}

async fn lift(world: &GraphWorld, moderator: &User, account_nanoid: &str, kind: Restriction) {
    match kind {
        Restriction::Suspend => world
            .module
            .unsuspend_account(&moderator.auth_account_id, account_nanoid.to_string())
            .await
            .unwrap(),
        Restriction::Ban => world
            .module
            .unban_account(&moderator.auth_account_id, account_nanoid.to_string())
            .await
            .unwrap(),
    }
    project(world).await;
}

async fn run(kind: Restriction) {
    let world = GraphWorld::new().await;
    world.allow_all_permissions().await;
    let owner = world.new_user().await;
    let moderator = world.new_user().await;
    let profile: LocalProfile = world.create_profile(&owner, true).await;
    let account_nanoid: String = sqlx::query_scalar(
        "SELECT a.nanoid FROM accounts a JOIN profiles p ON p.account_id = a.id WHERE p.nanoid = $1",
    )
    .bind(&profile.nanoid)
    .fetch_one(world.pool())
    .await
    .unwrap();
    let username: String = sqlx::query_scalar("SELECT username FROM profiles WHERE id = $1")
        .bind(profile.id.as_ref())
        .fetch_one(world.pool())
        .await
        .unwrap();
    let (key_id, key_pem): (String, String) = sqlx::query_as(
        "SELECT key_id_uri, public_key_pem FROM profile_signing_keys WHERE profile_id = $1",
    )
    .bind(profile.id.as_ref())
    .fetch_one(world.pool())
    .await
    .unwrap();

    // Before: an actor, resolvable, and a Follow to a remote actor is queued (the worker is not
    // running yet).
    assert_eq!(
        call(
            &world,
            "GET",
            &format!("/ap/profiles/{}", profile.nanoid),
            None,
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(webfinger(&world, &username).await, StatusCode::OK);
    let peer = MockServer::start().await;
    let remote_actor = format!("{}/users/bob", peer.uri());
    Mock::given(method("GET"))
        .and(path("/users/bob"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": remote_actor,
            "type": "Person",
            "inbox": format!("{}/inbox", peer.uri()),
        })))
        .mount(&peer)
        .await;
    Mock::given(method("POST"))
        .and(path("/inbox"))
        .respond_with(ResponseTemplate::new(202))
        .mount(&peer)
        .await;
    world
        .module
        .clone()
        .with_loopback_profile_delivery()
        .follow_profile(&owner.auth_account_id, None, &profile.nanoid, &remote_actor)
        .await
        .unwrap();
    assert_eq!(
        count(&world, "SELECT count(*) FROM profile_delivery_jobs").await,
        1
    );
    let follows_before = count(&world, "SELECT count(*) FROM profile_follows").await;

    // During the restriction.
    restrict(&world, &moderator, &account_nanoid, kind).await;
    assert_eq!(
        call(
            &world,
            "GET",
            &format!("/ap/profiles/{}", profile.nanoid),
            None,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND,
        "no actor document"
    );
    assert_eq!(
        webfinger(&world, &username).await,
        StatusCode::NOT_FOUND,
        "no WebFinger"
    );
    let fan = RemotePeer::new();
    let follow = fan.follow(&fan.activity_id("blocked"), &profile.actor_url);
    assert_eq!(
        fan.post_via_root(&world, &profile.nanoid, &follow).await,
        StatusCode::NOT_FOUND,
        "no inbox"
    );
    let (status, body) = call_loopback(
        &world,
        "POST",
        &format!("/api/v1/profiles/{}/follows", profile.nanoid),
        Some(&owner.token),
        Some(json!({"target": remote_actor})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "no new outgoing Follow: {body}"
    );
    // Another local Profile cannot follow it either (it is not an actor).
    let other_owner = world.new_user().await;
    let other = world.create_profile(&other_owner, true).await;
    let (status, body) = call(
        &world,
        "POST",
        &format!("/api/v1/profiles/{}/follows", other.nanoid),
        Some(&other_owner.token),
        Some(json!({"target": profile.actor_url})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        count(&world, "SELECT count(*) FROM profile_follows").await,
        follows_before,
        "no relation was created"
    );
    assert_eq!(
        count(&world, "SELECT count(*) FROM profile_delivery_jobs").await,
        1,
        "no delivery job was created"
    );
    // The queued Follow is not sent and no attempt is consumed.
    run_worker(&world, Duration::from_millis(1500)).await;
    assert!(
        posts_to(&peer).await.is_empty(),
        "nothing is delivered while restricted"
    );
    assert_eq!(
        count(
            &world,
            "SELECT count(*) FROM profile_delivery_jobs WHERE state = 'pending' AND attempt_count = 0"
        )
        .await,
        1
    );

    // After the restriction: the same identity and key; the queued Follow goes out.
    lift(&world, &moderator, &account_nanoid, kind).await;
    let (status, actor) = call(
        &world,
        "GET",
        &format!("/ap/profiles/{}", profile.nanoid),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(actor["id"], profile.actor_url.as_str());
    assert_eq!(actor["publicKey"]["id"], key_id.as_str());
    assert_eq!(actor["publicKey"]["publicKeyPem"], key_pem.as_str());
    assert_eq!(webfinger(&world, &username).await, StatusCode::OK);
    let follow = fan.follow(&fan.activity_id("allowed"), &profile.actor_url);
    assert!(
        fan.post_via_root(&world, &profile.nanoid, &follow)
            .await
            .is_success(),
        "the inbox works again"
    );
    assert_eq!(
        count(&world, "SELECT count(*) FROM profile_follows").await,
        follows_before + 1
    );
    let mut delivered = false;
    for _ in 0..10 {
        run_worker(&world, Duration::from_millis(600)).await;
        if !posts_to(&peer).await.is_empty() {
            delivered = true;
            break;
        }
    }
    assert!(
        delivered,
        "the queued Follow is delivered after the restriction"
    );
    let requests = posts_to(&peer).await;
    assert_eq!(requests.len(), 1);
    let mut verifier = HttpSignatureVerifierImpl::new().unwrap();
    verifier.cache_actor_key(&key_id, key_pem.clone());
    let headers: HashMap<String, String> = requests[0]
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                value.to_str().unwrap().to_string(),
            )
        })
        .collect();
    let verified = verifier
        .verify(&HttpSignatureVerificationInput {
            method: "POST".to_string(),
            url: format!("{}/inbox", peer.uri()),
            headers,
            body: Some(requests[0].body.clone()),
        })
        .await
        .unwrap();
    assert_eq!(verified, SignatureVerificationResult::Valid { key_id });
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_suspended_profile_is_not_an_actor_and_comes_back_unchanged() {
    run(Restriction::Suspend).await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_banned_profile_is_not_an_actor_and_comes_back_unchanged_after_unban() {
    run(Restriction::Ban).await;
}
