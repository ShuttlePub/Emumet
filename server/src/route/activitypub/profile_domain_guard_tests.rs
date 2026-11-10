//! D8G: domain `block` applied to the Profile inbox (Follow refusal, stale Accept, clean-up still
//! allowed, exact match, mute, Profile isolation, races with a block write).
//!
//! Same harness as the P3i tests: isolated database (production migrations + the P2 fixture),
//! signed requests, fake peers whose host is `peer-<uuid>.remote.example` unless a test picks one.

use crate::handler::AppModule;
use crate::route::profile_graph_test_support::{GraphWorld, LocalProfile, RemotePeer};
use application::service::domain_follow_guard::lock_profile_for_domain_control_write;
use axum::http::StatusCode;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnDatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
use kernel::interfaces::repository::{DependOnDomainControlRepository, DomainControlRepository};
use kernel::prelude::entity::{
    AccountKind, ControlDomain, DomainControlMode, GraphActorUrl, ProfileGraphActor,
};
use serde_json::{json, Value};
use sqlx::Executor;

const ACCEPTED: StatusCode = StatusCode::ACCEPTED;

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .unwrap()
        .host_str()
        .unwrap()
        .to_string()
}

async fn set_control(
    world: &GraphWorld,
    profile: &LocalProfile,
    domain: &str,
    mode: DomainControlMode,
) {
    let mut conn = world
        .module
        .database_connection()
        .connection()
        .await
        .unwrap();
    world
        .module
        .domain_control_repository()
        .create(
            &mut conn,
            &profile.id,
            &ControlDomain::parse(domain).unwrap(),
            mode,
        )
        .await
        .unwrap()
        .expect("control created");
}

async fn clear_controls(world: &GraphWorld) {
    world
        .pool()
        .execute("DELETE FROM profile_domain_controls")
        .await
        .unwrap();
}

async fn state_of(world: &GraphWorld, activity_id: &str) -> String {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT state, end_reason FROM profile_follows WHERE activity_id = $1")
            .bind(activity_id)
            .fetch_optional(world.pool())
            .await
            .unwrap();
    match row {
        Some((state, None)) => state,
        Some((state, Some(reason))) => format!("{state}:{reason}"),
        None => "none".to_string(),
    }
}

async fn count(world: &GraphWorld, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(world.pool())
        .await
        .unwrap()
}

async fn jobs(world: &GraphWorld, activity_type: &str) -> Vec<(String, String, Option<i64>)> {
    sqlx::query_as(
        "SELECT target_inbox, payload, follow_id FROM profile_delivery_jobs WHERE activity_type = $1 ORDER BY id",
    )
    .bind(activity_type)
    .fetch_all(world.pool())
    .await
    .unwrap()
}

async fn seed_outbound_follow(
    world: &GraphWorld,
    profile: &LocalProfile,
    peer: &RemotePeer,
    activity_id: &str,
    accepted: bool,
) {
    world
        .seed_follow(
            ProfileGraphActor::Local(profile.id.clone()),
            ProfileGraphActor::Remote(GraphActorUrl::new(peer.actor_url.clone()).unwrap()),
            activity_id,
            accepted,
        )
        .await;
}

fn our_follow(profile: &LocalProfile, peer: &RemotePeer, id: &str) -> Value {
    json!({"id": id, "type": "Follow", "actor": profile.actor_url, "object": peer.actor_url})
}

type Held = (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
);

/// A block write in flight: it holds the Profile relationship lock (taken the way
/// `put_domain_control` takes it) and has stored a domain block that is not committed yet.
fn domain_block_write_in_flight(world: &GraphWorld, profile: &LocalProfile, domain: &str) -> Held {
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let module: AppModule = world.module.clone();
    let profile_id = profile.id.clone();
    let domain = ControlDomain::parse(domain).unwrap();
    let holder = tokio::spawn(async move {
        let deps = module.clone();
        module
            .transaction_manager()
            .transaction(move |executor| {
                Box::pin(async move {
                    lock_profile_for_domain_control_write(&deps, executor, &profile_id).await?;
                    deps.domain_control_repository()
                        .create(executor, &profile_id, &domain, DomainControlMode::Block)
                        .await?;
                    locked_tx.send(()).ok();
                    release_rx.await.ok();
                    Ok(())
                })
            })
            .await
            .unwrap();
    });
    (locked_rx, release_tx, holder)
}

async fn still_waiting<T>(handle: &mut tokio::task::JoinHandle<T>) -> bool {
    tokio::time::timeout(std::time::Duration::from_millis(500), handle)
        .await
        .is_err()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn follow_from_a_domain_blocked_actor_is_refused_with_a_reject_and_never_revived() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let sibling_profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let friend = RemotePeer::new();
    set_control(
        &world,
        &profile,
        &host_of(&peer.actor_url),
        DomainControlMode::Block,
    )
    .await;
    let blocked_follow = peer.activity_id("follow-1");
    let body = peer.follow(&blocked_follow, &profile.actor_url);

    assert_eq!(peer.post(&world, &profile.nanoid, &body).await, ACCEPTED);

    // No active relation and no Accept: the refusal is an ended(blocked) generation + a Reject.
    assert_eq!(state_of(&world, &blocked_follow).await, "ended:blocked");
    assert_eq!(
        count(&world, "profile_follows WHERE state <> 'ended'").await,
        0
    );
    assert!(jobs(&world, "Accept").await.is_empty());
    let rejects = jobs(&world, "Reject").await;
    assert_eq!(rejects.len(), 1);
    assert_eq!(rejects[0].0, peer.inbox_url);
    let payload: Value = serde_json::from_str(&rejects[0].1).unwrap();
    assert_eq!(payload["type"], "Reject");
    assert_eq!(payload["actor"], profile.actor_url.as_str());
    assert_eq!(payload["to"][0], peer.actor_url.as_str());
    assert_eq!(payload["object"]["id"], blocked_follow.as_str());
    assert_eq!(payload["object"]["type"], "Follow");
    assert!(
        !payload.to_string().contains("\"Block\""),
        "an AP Block is never sent"
    );
    assert!(rejects[0].2.is_some());

    // A replay is a duplicate and reserves no second Reject.
    assert_eq!(peer.post(&world, &profile.nanoid, &body).await, ACCEPTED);
    assert_eq!(jobs(&world, "Reject").await.len(), 1);

    // The policy belongs to this Profile: another Profile and another domain are unaffected.
    let sibling_follow = peer.activity_id("follow-2");
    assert_eq!(
        peer.post(
            &world,
            &sibling_profile.nanoid,
            &peer.follow(&sibling_follow, &sibling_profile.actor_url)
        )
        .await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &sibling_follow).await, "accepted");
    let friend_follow = friend.activity_id("follow-1");
    assert_eq!(
        friend
            .post(
                &world,
                &profile.nanoid,
                &friend.follow(&friend_follow, &profile.actor_url)
            )
            .await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &friend_follow).await, "accepted");

    // Unblocking restores nothing: the old Follow stays refused, a new Follow is a new decision.
    clear_controls(&world).await;
    assert_eq!(peer.post(&world, &profile.nanoid, &body).await, ACCEPTED);
    assert_eq!(state_of(&world, &blocked_follow).await, "ended:blocked");
    assert_eq!(jobs(&world, "Reject").await.len(), 1);
    let later = peer.activity_id("follow-3");
    assert_eq!(
        peer.post(
            &world,
            &profile.nanoid,
            &peer.follow(&later, &profile.actor_url)
        )
        .await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &later).await, "accepted");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn matching_is_exact_on_the_actor_id_host_and_never_on_the_inbox_host() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;

    // Block `example-d8g.net`: a sub-domain actor is not blocked.
    set_control(
        &world,
        &profile,
        "example-d8g.net",
        DomainControlMode::Block,
    )
    .await;
    let sub = RemotePeer::at("https://sub.example-d8g.net/users/sub");
    let sub_follow = sub.activity_id("follow");
    assert_eq!(
        sub.post(
            &world,
            &profile.nanoid,
            &sub.follow(&sub_follow, &profile.actor_url)
        )
        .await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &sub_follow).await, "accepted");
    // The blocked domain itself is.
    let exact = RemotePeer::at("https://example-d8g.net/users/exact");
    let exact_follow = exact.activity_id("follow");
    assert_eq!(
        exact
            .post(
                &world,
                &profile.nanoid,
                &exact.follow(&exact_follow, &profile.actor_url)
            )
            .await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &exact_follow).await, "ended:blocked");

    // Block `sub.parent-d8g.net`: the parent domain is not blocked.
    set_control(
        &world,
        &profile,
        "sub.parent-d8g.net",
        DomainControlMode::Block,
    )
    .await;
    let parent = RemotePeer::at("https://parent-d8g.net/users/parent");
    let parent_follow = parent.activity_id("follow");
    assert_eq!(
        parent
            .post(
                &world,
                &profile.nanoid,
                &parent.follow(&parent_follow, &profile.actor_url)
            )
            .await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &parent_follow).await, "accepted");

    // The inbox / CDN host is never consulted: an actor on a clean domain whose inbox lives on a
    // blocked domain is accepted, and the other way round is refused.
    set_control(
        &world,
        &profile,
        "cdn-d8g.example",
        DomainControlMode::Block,
    )
    .await;
    let mut clean_actor = RemotePeer::at("https://clean-d8g.example/users/clean");
    clean_actor.inbox_url = "https://cdn-d8g.example/inbox".to_string();
    clean_actor.register_actor_document();
    let clean_follow = clean_actor.activity_id("follow");
    assert_eq!(
        clean_actor
            .post(
                &world,
                &profile.nanoid,
                &clean_actor.follow(&clean_follow, &profile.actor_url)
            )
            .await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &clean_follow).await, "accepted");
    let mut blocked_actor = RemotePeer::at("https://cdn-d8g.example/users/blocked");
    blocked_actor.inbox_url = "https://clean-d8g.example/inbox".to_string();
    blocked_actor.register_actor_document();
    let blocked_follow = blocked_actor.activity_id("follow");
    assert_eq!(
        blocked_actor
            .post(
                &world,
                &profile.nanoid,
                &blocked_actor.follow(&blocked_follow, &profile.actor_url)
            )
            .await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &blocked_follow).await, "ended:blocked");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn mute_does_not_refuse_a_follow_or_an_accept() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    set_control(
        &world,
        &profile,
        &host_of(&peer.actor_url),
        DomainControlMode::Mute,
    )
    .await;

    let follow_id = peer.activity_id("follow-1");
    assert_eq!(
        peer.post(
            &world,
            &profile.nanoid,
            &peer.follow(&follow_id, &profile.actor_url)
        )
        .await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &follow_id).await, "accepted");
    assert_eq!(jobs(&world, "Accept").await.len(), 1);
    assert!(jobs(&world, "Reject").await.is_empty());

    let ours = "https://local.example/follows/mute-1";
    seed_outbound_follow(&world, &profile, &peer, ours, false).await;
    let accept = peer.respond(
        "Accept",
        &peer.activity_id("accept-1"),
        our_follow(&profile, &peer, ours),
    );
    assert_eq!(peer.post(&world, &profile.nanoid, &accept).await, ACCEPTED);
    assert_eq!(state_of(&world, ours).await, "accepted");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_late_accept_from_a_blocked_domain_does_not_activate_the_follow() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let ours = "https://local.example/follows/late-1";
    seed_outbound_follow(&world, &profile, &peer, ours, false).await;
    set_control(
        &world,
        &profile,
        &host_of(&peer.actor_url),
        DomainControlMode::Block,
    )
    .await;

    let accept = peer.respond(
        "Accept",
        &peer.activity_id("accept-1"),
        our_follow(&profile, &peer, ours),
    );
    assert_eq!(peer.post(&world, &profile.nanoid, &accept).await, ACCEPTED);
    assert_eq!(state_of(&world, ours).await, "pending", "stays pending");
    assert_eq!(
        count(&world, "profile_inbound_activities").await,
        1,
        "recorded"
    );

    // Replaying the same Accept after the unblock is a duplicate: the relation does not appear.
    clear_controls(&world).await;
    assert_eq!(peer.post(&world, &profile.nanoid, &accept).await, ACCEPTED);
    assert_eq!(state_of(&world, ours).await, "pending");
    assert_eq!(
        count(&world, "profile_follows WHERE state = 'accepted'").await,
        0
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn undo_and_reject_clean_up_is_processed_while_the_domain_is_blocked() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();

    // An accepted incoming Follow and an accepted / pending outgoing Follow exist before the block.
    let incoming = peer.activity_id("follow-1");
    assert_eq!(
        peer.post(
            &world,
            &profile.nanoid,
            &peer.follow(&incoming, &profile.actor_url)
        )
        .await,
        ACCEPTED
    );
    let ours_accepted = "https://local.example/follows/cleanup-1";
    let ours_pending = "https://local.example/follows/cleanup-2";
    let other_peer = peer.clone();
    seed_outbound_follow(&world, &profile, &peer, ours_accepted, true).await;
    set_control(
        &world,
        &profile,
        &host_of(&peer.actor_url),
        DomainControlMode::Block,
    )
    .await;

    let undo = peer.undo(
        &peer.activity_id("undo-1"),
        json!({"id": incoming, "type": "Follow", "actor": peer.actor_url, "object": profile.actor_url}),
    );
    assert_eq!(peer.post(&world, &profile.nanoid, &undo).await, ACCEPTED);
    assert_eq!(state_of(&world, &incoming).await, "ended:undone");

    let reject = peer.respond(
        "Reject",
        &peer.activity_id("reject-1"),
        our_follow(&profile, &peer, ours_accepted),
    );
    assert_eq!(peer.post(&world, &profile.nanoid, &reject).await, ACCEPTED);
    assert_eq!(state_of(&world, ours_accepted).await, "ended:rejected");

    // A pending outgoing Follow created while the block exists is also ended by a Reject.
    clear_controls(&world).await;
    seed_outbound_follow(&world, &profile, &other_peer, ours_pending, false).await;
    set_control(
        &world,
        &profile,
        &host_of(&peer.actor_url),
        DomainControlMode::Block,
    )
    .await;
    let reject = other_peer.respond(
        "Reject",
        &other_peer.activity_id("reject-2"),
        our_follow(&profile, &other_peer, ours_pending),
    );
    assert_eq!(
        other_peer.post(&world, &profile.nanoid, &reject).await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, ours_pending).await, "ended:rejected");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn the_policy_stays_with_the_profile_after_its_owner_changes() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    set_control(
        &world,
        &profile,
        &host_of(&peer.actor_url),
        DomainControlMode::Block,
    )
    .await;

    // Hand the Profile to an organization Account (the same UPDATE a transfer projects).
    let org = kernel::test_utils::AccountBuilder::new()
        .kind(AccountKind::Organization)
        .name(kernel::test_utils::unique_account_name())
        .build();
    {
        let mut conn = world
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        world
            .module
            .account_read_model()
            .create(&mut conn, &org)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE profiles SET account_id = $1 WHERE id = $2")
        .bind(org.id().as_ref())
        .bind(profile.id.as_ref())
        .execute(world.pool())
        .await
        .unwrap();

    let follow_id = peer.activity_id("follow-1");
    assert_eq!(
        peer.post(
            &world,
            &profile.nanoid,
            &peer.follow(&follow_id, &profile.actor_url)
        )
        .await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &follow_id).await, "ended:blocked");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn follow_waits_for_an_in_flight_domain_block_write_and_is_then_refused() {
    let world = std::sync::Arc::new(GraphWorld::new().await);
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let follow_id = peer.activity_id("follow-1");

    let (locked, release, holder) =
        domain_block_write_in_flight(&world, &profile, &host_of(&peer.actor_url));
    locked.await.unwrap();

    let mut follow = tokio::spawn({
        let world = world.clone();
        let peer = peer.clone();
        let body = peer.follow(&follow_id, &profile.actor_url);
        let nanoid = profile.nanoid.clone();
        async move { peer.post(&world, &nanoid, &body).await }
    });
    assert!(
        still_waiting(&mut follow).await,
        "the Follow must wait for the block write instead of racing it"
    );
    assert_eq!(count(&world, "profile_follows").await, 0);

    release.send(()).unwrap();
    holder.await.unwrap();
    assert_eq!(follow.await.unwrap(), ACCEPTED);
    assert_eq!(state_of(&world, &follow_id).await, "ended:blocked");
    assert!(jobs(&world, "Accept").await.is_empty());
    assert_eq!(jobs(&world, "Reject").await.len(), 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_accept_waits_for_an_in_flight_domain_block_write_and_is_then_not_applied() {
    let world = std::sync::Arc::new(GraphWorld::new().await);
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let ours = "https://local.example/follows/race-1";
    seed_outbound_follow(&world, &profile, &peer, ours, false).await;

    let (locked, release, holder) =
        domain_block_write_in_flight(&world, &profile, &host_of(&peer.actor_url));
    locked.await.unwrap();
    let mut accept = tokio::spawn({
        let world = world.clone();
        let peer = peer.clone();
        let body = peer.respond(
            "Accept",
            &peer.activity_id("accept-1"),
            our_follow(&profile, &peer, ours),
        );
        let nanoid = profile.nanoid.clone();
        async move { peer.post(&world, &nanoid, &body).await }
    });
    assert!(still_waiting(&mut accept).await);
    assert_eq!(state_of(&world, ours).await, "pending");

    release.send(()).unwrap();
    holder.await.unwrap();
    assert_eq!(accept.await.unwrap(), ACCEPTED);
    assert_eq!(state_of(&world, ours).await, "pending");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_follows_from_a_blocked_domain_leave_one_refusal_and_no_relation() {
    let world = std::sync::Arc::new(GraphWorld::new().await);
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    set_control(
        &world,
        &profile,
        &host_of(&peer.actor_url),
        DomainControlMode::Block,
    )
    .await;
    let follow_id = peer.activity_id("follow-1");
    let body = peer.follow(&follow_id, &profile.actor_url);

    let mut handles = Vec::new();
    for _ in 0..6 {
        handles.push(tokio::spawn({
            let world = world.clone();
            let peer = peer.clone();
            let body = body.clone();
            let nanoid = profile.nanoid.clone();
            async move { peer.post(&world, &nanoid, &body).await }
        }));
    }
    for handle in handles {
        assert_eq!(handle.await.unwrap(), ACCEPTED);
    }
    assert_eq!(count(&world, "profile_follows").await, 1);
    assert_eq!(state_of(&world, &follow_id).await, "ended:blocked");
    assert!(jobs(&world, "Accept").await.is_empty());
    assert_eq!(jobs(&world, "Reject").await.len(), 1);
}
