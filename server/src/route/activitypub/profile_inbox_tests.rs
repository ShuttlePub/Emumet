//! Profile inbox over HTTP with signed requests and a real database (P3i).
//!
//! Every test runs in its own isolated database (production migrations + the P2 schema fixture)
//! and mounts the inbox on a test-only router, exactly the way P4 will mount it. Remote peers are
//! fake actors whose keys / actor documents come from the test-mode caches.

use crate::route::profile_graph_test_support::{
    peer_private_pem, GraphWorld, LocalProfile, RemotePeer,
};
use axum::http::StatusCode;
use kernel::prelude::entity::{GraphActorUrl, ProfileGraphActor};
use serde_json::{json, Value};
use sqlx::Executor;

const ACCEPTED: StatusCode = StatusCode::ACCEPTED;

async fn follow_row(world: &GraphWorld, activity_id: &str) -> Option<(String, Option<String>)> {
    sqlx::query_as("SELECT state, end_reason FROM profile_follows WHERE activity_id = $1")
        .bind(activity_id)
        .fetch_optional(world.pool())
        .await
        .unwrap()
}

async fn state_of(world: &GraphWorld, activity_id: &str) -> String {
    let row = follow_row(world, activity_id).await;
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

/// A Follow this Profile sent to `peer` (stored by the P2 repository, as P3o will).
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

fn embedded_follow(actor: &str, id: &str, target: &str) -> Value {
    json!({"id": id, "type": "Follow", "actor": actor, "object": target})
}

/// The Follow this Profile sent to `peer`, as the peer embeds it in its Accept / Reject.
fn our_follow(profile: &LocalProfile, peer: &RemotePeer, id: &str) -> Value {
    embedded_follow(&profile.actor_url, id, &peer.actor_url)
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn follow_stores_an_accepted_generation_and_reserves_the_accept_in_one_transaction() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let follow_id = peer.activity_id("follow-1");

    let status = peer
        .post(
            &world,
            &profile.nanoid,
            &peer.follow(&follow_id, &profile.actor_url),
        )
        .await;
    assert_eq!(status, ACCEPTED);

    assert_eq!(state_of(&world, &follow_id).await, "accepted");
    let jobs = jobs(&world, "Accept").await;
    assert_eq!(jobs.len(), 1);
    let (target_inbox, payload, follow_fk) = &jobs[0];
    assert_eq!(
        target_inbox, &peer.inbox_url,
        "Accept goes to the follower's own inbox"
    );
    assert!(follow_fk.is_some(), "the job is linked to the generation");
    let payload: Value = serde_json::from_str(payload).unwrap();
    assert_eq!(payload["type"], "Accept");
    assert_eq!(payload["actor"], profile.actor_url.as_str());
    assert_eq!(payload["to"][0], peer.actor_url.as_str());
    assert_eq!(payload["object"]["id"], follow_id.as_str());
    assert_eq!(payload["object"]["type"], "Follow");
    assert_eq!(payload["object"]["actor"], peer.actor_url.as_str());
    assert_eq!(payload["object"]["object"], profile.actor_url.as_str());
    assert_eq!(count(&world, "profile_inbound_activities").await, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn duplicate_follow_is_detected_and_creates_nothing_more() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let follow_id = peer.activity_id("follow-1");
    let body = peer.follow(&follow_id, &profile.actor_url);

    for _ in 0..3 {
        assert_eq!(peer.post(&world, &profile.nanoid, &body).await, ACCEPTED);
    }
    assert_eq!(count(&world, "profile_follows").await, 1);
    assert_eq!(count(&world, "profile_delivery_jobs").await, 1);
    let receive_count: i32 =
        sqlx::query_scalar("SELECT receive_count FROM profile_inbound_activities")
            .fetch_one(world.pool())
            .await
            .unwrap();
    assert_eq!(
        receive_count, 3,
        "every delivery is counted by the P2 record"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_deliveries_of_one_follow_apply_once() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let body = peer.follow(&peer.activity_id("follow-1"), &profile.actor_url);

    let (a, b, c, d) = tokio::join!(
        peer.post(&world, &profile.nanoid, &body),
        peer.post(&world, &profile.nanoid, &body),
        peer.post(&world, &profile.nanoid, &body),
        peer.post(&world, &profile.nanoid, &body),
    );
    for status in [a, b, c, d] {
        assert_eq!(status, ACCEPTED);
    }
    assert_eq!(count(&world, "profile_follows").await, 1);
    assert_eq!(count(&world, "profile_delivery_jobs").await, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_new_follow_replaces_only_the_older_generation_of_the_same_actor() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let other_profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let bystander = RemotePeer::new();
    let first = peer.activity_id("follow-1");
    let second = peer.activity_id("follow-2");
    let bystander_follow = bystander.activity_id("follow-1");

    for (who, id, target) in [
        (&peer, &first, &profile),
        (&bystander, &bystander_follow, &profile),
        (&peer, &first, &other_profile),
    ] {
        let status = who
            .post(&world, &target.nanoid, &who.follow(id, &target.actor_url))
            .await;
        assert_eq!(status, ACCEPTED);
    }
    // The third request reuses the first activity id for another Profile: it never overwrites
    // the generation of the first one.
    assert_eq!(state_of(&world, &first).await, "accepted");

    assert_eq!(
        peer.post(
            &world,
            &profile.nanoid,
            &peer.follow(&second, &profile.actor_url)
        )
        .await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &first).await, "ended:undone");
    assert_eq!(state_of(&world, &second).await, "accepted");
    assert_eq!(
        state_of(&world, &bystander_follow).await,
        "accepted",
        "another actor's follow is untouched"
    );
    assert_eq!(jobs(&world, "Accept").await.len(), 3);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn requests_that_fail_verification_change_nothing() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let impostor = RemotePeer::new();
    let follow = peer.follow(&peer.activity_id("follow-1"), &profile.actor_url);

    // Signed with a key that is not the peer's.
    assert_eq!(
        peer.post_with_stranger_key(&world, &profile.nanoid, &follow)
            .await,
        StatusCode::UNAUTHORIZED
    );
    // No signature at all.
    assert_eq!(
        peer.post_raw(
            &world,
            &profile.nanoid,
            serde_json::to_vec(&follow).unwrap(),
            None,
            None
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    // Body changed after signing (digest mismatch).
    let tampered = peer.follow(&peer.activity_id("follow-9"), &profile.actor_url);
    assert_eq!(
        peer.post_raw(
            &world,
            &profile.nanoid,
            serde_json::to_vec(&follow).unwrap(),
            Some((peer_private_pem(), &peer.key_id)),
            Some(serde_json::to_vec(&tampered).unwrap()),
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    // A valid signature by `impostor`, but the activity names `peer` as the actor.
    assert_eq!(
        impostor.post(&world, &profile.nanoid, &follow).await,
        StatusCode::UNAUTHORIZED
    );
    // The key's owner is not the activity actor even though the keyId looks like the peer's.
    let mismatched_key = format!("{}#main-key", impostor.actor_url);
    assert_eq!(
        peer.post_as(
            &world,
            &profile.nanoid,
            &follow,
            peer_private_pem(),
            &mismatched_key
        )
        .await,
        StatusCode::UNAUTHORIZED
    );

    assert_eq!(count(&world, "profile_follows").await, 0);
    assert_eq!(count(&world, "profile_delivery_jobs").await, 0);
    assert_eq!(count(&world, "profile_inbound_activities").await, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn follow_for_a_missing_non_actor_or_other_profile_is_refused() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let other = world.actor_profile().await;
    let user = world.new_user().await;
    let not_an_actor = world.create_profile(&user, false).await;
    let peer = RemotePeer::new();
    let follow = |target: &str| peer.follow(&peer.activity_id("follow-1"), target);

    assert_eq!(
        peer.post(&world, "no-such-profile", &follow(&profile.actor_url))
            .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        peer.post(
            &world,
            &not_an_actor.nanoid,
            &follow(&not_an_actor.actor_url)
        )
        .await,
        StatusCode::NOT_FOUND,
        "a Profile without username / key is not an actor and has no inbox"
    );
    // Delivered to `profile`'s inbox but aimed at `other`.
    assert_eq!(
        peer.post(&world, &profile.nanoid, &follow(&other.actor_url))
            .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Not a Profile actor at all.
    assert_eq!(
        peer.post(
            &world,
            &profile.nanoid,
            &follow("https://elsewhere.example/u/x")
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(count(&world, "profile_follows").await, 0);
    assert_eq!(count(&world, "profile_delivery_jobs").await, 0);
    assert_eq!(count(&world, "profile_inbound_activities").await, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_unresolvable_follower_is_retryable_and_leaves_no_trace() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::unresolved();
    let body = peer.follow(&peer.activity_id("follow-1"), &profile.actor_url);

    let status = peer.post(&world, &profile.nanoid, &body).await;
    assert_eq!(status, StatusCode::REQUEST_TIMEOUT);
    assert_eq!(count(&world, "profile_follows").await, 0);
    assert_eq!(count(&world, "profile_inbound_activities").await, 0);

    peer.register_actor_document();
    assert_eq!(peer.post(&world, &profile.nanoid, &body).await, ACCEPTED);
    assert_eq!(count(&world, "profile_follows").await, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn local_actor_urls_cannot_deliver_over_http() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::at(&format!("{}/ap/accounts/imposter", world.base_url));
    let status = peer
        .post(
            &world,
            &profile.nanoid,
            &peer.follow(&peer.activity_id("follow-1"), &profile.actor_url),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(count(&world, "profile_follows").await, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn undo_ends_the_generation_and_a_replay_or_stale_follow_cannot_revive_it() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let follow_id = peer.activity_id("follow-1");
    let follow = peer.follow(&follow_id, &profile.actor_url);
    assert_eq!(peer.post(&world, &profile.nanoid, &follow).await, ACCEPTED);

    let undo = peer.undo(
        &peer.activity_id("undo-1"),
        embedded_follow(&peer.actor_url, &follow_id, &profile.actor_url),
    );
    assert_eq!(peer.post(&world, &profile.nanoid, &undo).await, ACCEPTED);
    assert_eq!(state_of(&world, &follow_id).await, "ended:undone");

    // Replays of both activities: nothing is revived, nothing new is reserved.
    assert_eq!(peer.post(&world, &profile.nanoid, &undo).await, ACCEPTED);
    assert_eq!(peer.post(&world, &profile.nanoid, &follow).await, ACCEPTED);
    assert_eq!(state_of(&world, &follow_id).await, "ended:undone");
    assert_eq!(count(&world, "profile_follows").await, 1);
    assert_eq!(jobs(&world, "Accept").await.len(), 1);

    // The same Follow under a fresh Undo id (the dedupe record does not hide the generation).
    let undo_again = peer.undo(
        &peer.activity_id("undo-2"),
        Value::String(follow_id.clone()),
    );
    assert_eq!(
        peer.post(&world, &profile.nanoid, &undo_again).await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &follow_id).await, "ended:undone");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn undo_before_follow_remembers_the_generation_so_the_late_follow_creates_nothing() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let follow_id = peer.activity_id("follow-1");

    let undo = peer.undo(
        &peer.activity_id("undo-1"),
        embedded_follow(&peer.actor_url, &follow_id, &profile.actor_url),
    );
    assert_eq!(peer.post(&world, &profile.nanoid, &undo).await, ACCEPTED);
    assert_eq!(state_of(&world, &follow_id).await, "ended:undone");

    let follow = peer.follow(&follow_id, &profile.actor_url);
    assert_eq!(peer.post(&world, &profile.nanoid, &follow).await, ACCEPTED);
    assert_eq!(state_of(&world, &follow_id).await, "ended:undone");
    assert_eq!(count(&world, "profile_delivery_jobs").await, 0);

    // A genuinely new Follow (new activity id) works afterwards.
    let again = peer.activity_id("follow-2");
    assert_eq!(
        peer.post(
            &world,
            &profile.nanoid,
            &peer.follow(&again, &profile.actor_url)
        )
        .await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &again).await, "accepted");
    assert_eq!(jobs(&world, "Accept").await.len(), 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn another_actor_cannot_undo_somebody_elses_follow() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let victim = RemotePeer::new();
    let attacker = RemotePeer::new();
    let follow_id = victim.activity_id("follow-1");
    assert_eq!(
        victim
            .post(
                &world,
                &profile.nanoid,
                &victim.follow(&follow_id, &profile.actor_url)
            )
            .await,
        ACCEPTED
    );

    // Embedded Follow names the victim as its actor, but the attacker sends the Undo.
    let embedded = attacker.undo(
        &attacker.activity_id("undo-1"),
        embedded_follow(&victim.actor_url, &follow_id, &profile.actor_url),
    );
    assert_eq!(
        attacker.post(&world, &profile.nanoid, &embedded).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Bare id of the victim's Follow: looked up in the attacker's own namespace.
    let bare = attacker.undo(
        &attacker.activity_id("undo-2"),
        Value::String(follow_id.clone()),
    );
    assert_eq!(
        attacker.post(&world, &profile.nanoid, &bare).await,
        ACCEPTED
    );
    assert_eq!(
        state_of(&world, &follow_id).await,
        "accepted",
        "the victim's relation survives"
    );
    let followers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM profile_follows WHERE follower_actor_url = $1 AND state <> 'ended'",
    )
    .bind(&victim.actor_url)
    .fetch_one(world.pool())
    .await
    .unwrap();
    assert_eq!(followers, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn undo_for_another_profile_does_not_touch_this_profiles_follow() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let other = world.actor_profile().await;
    let peer = RemotePeer::new();
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

    // The Undo is addressed to `other`, whose inbox it hits, while the Follow belongs to `profile`.
    let bare = peer.undo(
        &peer.activity_id("undo-1"),
        Value::String(follow_id.clone()),
    );
    assert_eq!(peer.post(&world, &other.nanoid, &bare).await, ACCEPTED);
    assert_eq!(state_of(&world, &follow_id).await, "accepted");

    let embedded = peer.undo(
        &peer.activity_id("undo-2"),
        embedded_follow(&peer.actor_url, &follow_id, &profile.actor_url),
    );
    assert_eq!(
        peer.post(&world, &other.nanoid, &embedded).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(state_of(&world, &follow_id).await, "accepted");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn accept_activates_only_the_matching_pending_generation() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let other_profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let follow_id = format!("{}/follows/1", profile.actor_url);
    let other_follow_id = format!("{}/follows/1", other_profile.actor_url);
    seed_outbound_follow(&world, &profile, &peer, &follow_id, false).await;
    seed_outbound_follow(&world, &other_profile, &peer, &other_follow_id, false).await;

    // Accept for an unknown Follow id, or a Follow of another actor's namespace: no effect.
    for unknown in ["https://l.example/follows/404", "not a url"] {
        let accept = peer.respond(
            "Accept",
            &peer.activity_id(&format!("accept-unknown-{}", unknown.len())),
            our_follow(&profile, &peer, unknown),
        );
        assert_eq!(peer.post(&world, &profile.nanoid, &accept).await, ACCEPTED);
    }
    assert_eq!(state_of(&world, &follow_id).await, "pending");

    // The Accept references the other Profile's Follow but arrives at `profile`'s inbox.
    let misdirected = peer.respond(
        "Accept",
        &peer.activity_id("accept-misdirected"),
        our_follow(&other_profile, &peer, &other_follow_id),
    );
    assert_eq!(
        peer.post(&world, &profile.nanoid, &misdirected).await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &other_follow_id).await, "pending");
    assert_eq!(state_of(&world, &follow_id).await, "pending");

    // A different actor cannot accept a Follow that was sent to the peer.
    let stranger = RemotePeer::new();
    let forged = stranger.respond(
        "Accept",
        &stranger.activity_id("accept-1"),
        our_follow(&profile, &peer, &follow_id),
    );
    assert_eq!(
        stranger.post(&world, &profile.nanoid, &forged).await,
        ACCEPTED
    );
    let forged_bare = stranger.respond(
        "Accept",
        &stranger.activity_id("accept-2"),
        Value::String(follow_id.clone()),
    );
    assert_eq!(
        stranger.post(&world, &profile.nanoid, &forged_bare).await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &follow_id).await, "pending");

    // The real Accept (embedded form) activates it; a replay and a bare-id Accept are harmless.
    let accept = peer.respond(
        "Accept",
        &peer.activity_id("accept-real"),
        our_follow(&profile, &peer, &follow_id),
    );
    assert_eq!(peer.post(&world, &profile.nanoid, &accept).await, ACCEPTED);
    assert_eq!(state_of(&world, &follow_id).await, "accepted");
    assert_eq!(peer.post(&world, &profile.nanoid, &accept).await, ACCEPTED);
    let bare = peer.respond(
        "Accept",
        &peer.activity_id("accept-bare"),
        Value::String(follow_id.clone()),
    );
    assert_eq!(peer.post(&world, &profile.nanoid, &bare).await, ACCEPTED);
    assert_eq!(state_of(&world, &follow_id).await, "accepted");
    assert_eq!(state_of(&world, &other_follow_id).await, "pending");
    assert_eq!(
        count(&world, "profile_delivery_jobs").await,
        0,
        "Accept / Reject reception reserves no delivery"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_stale_accept_for_an_ended_generation_leaves_the_new_generation_pending() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let old_id = format!("{}/follows/old", profile.actor_url);
    let new_id = format!("{}/follows/new", profile.actor_url);
    seed_outbound_follow(&world, &profile, &peer, &old_id, false).await;

    // The peer rejects the first Follow, then the profile follows again (new generation).
    let reject = peer.respond(
        "Reject",
        &peer.activity_id("reject-1"),
        our_follow(&profile, &peer, &old_id),
    );
    assert_eq!(peer.post(&world, &profile.nanoid, &reject).await, ACCEPTED);
    assert_eq!(state_of(&world, &old_id).await, "ended:rejected");
    seed_outbound_follow(&world, &profile, &peer, &new_id, false).await;

    // The old Accept (delayed in transit) must not activate anything.
    let stale = peer.respond(
        "Accept",
        &peer.activity_id("accept-old"),
        our_follow(&profile, &peer, &old_id),
    );
    assert_eq!(peer.post(&world, &profile.nanoid, &stale).await, ACCEPTED);
    assert_eq!(state_of(&world, &old_id).await, "ended:rejected");
    assert_eq!(state_of(&world, &new_id).await, "pending");

    // A stale Reject cannot end the new generation either.
    let stale_reject = peer.respond(
        "Reject",
        &peer.activity_id("reject-old"),
        our_follow(&profile, &peer, &old_id),
    );
    assert_eq!(
        peer.post(&world, &profile.nanoid, &stale_reject).await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &new_id).await, "pending");

    let accept = peer.respond(
        "Accept",
        &peer.activity_id("accept-new"),
        our_follow(&profile, &peer, &new_id),
    );
    assert_eq!(peer.post(&world, &profile.nanoid, &accept).await, ACCEPTED);
    assert_eq!(state_of(&world, &new_id).await, "accepted");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn reject_ends_pending_and_accepted_generations_only_for_the_followee() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let stranger = RemotePeer::new();
    let pending_id = format!("{}/follows/pending", profile.actor_url);
    seed_outbound_follow(&world, &profile, &peer, &pending_id, false).await;

    let forged = stranger.respond(
        "Reject",
        &stranger.activity_id("reject-1"),
        our_follow(&profile, &peer, &pending_id),
    );
    assert_eq!(
        stranger.post(&world, &profile.nanoid, &forged).await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &pending_id).await, "pending");

    let reject = peer.respond(
        "Reject",
        &peer.activity_id("reject-1"),
        our_follow(&profile, &peer, &pending_id),
    );
    assert_eq!(peer.post(&world, &profile.nanoid, &reject).await, ACCEPTED);
    assert_eq!(state_of(&world, &pending_id).await, "ended:rejected");

    // An accepted generation can be removed by the followee with a Reject as well.
    let accepted_id = format!("{}/follows/accepted", profile.actor_url);
    seed_outbound_follow(&world, &profile, &peer, &accepted_id, true).await;
    let remove = peer.respond(
        "Reject",
        &peer.activity_id("reject-2"),
        Value::String(accepted_id.clone()),
    );
    assert_eq!(peer.post(&world, &profile.nanoid, &remove).await, ACCEPTED);
    assert_eq!(state_of(&world, &accepted_id).await, "ended:rejected");

    // Nothing revives a rejected generation, not even a late Accept.
    let late_accept = peer.respond(
        "Accept",
        &peer.activity_id("accept-late"),
        our_follow(&profile, &peer, &pending_id),
    );
    assert_eq!(
        peer.post(&world, &profile.nanoid, &late_accept).await,
        ACCEPTED
    );
    assert_eq!(state_of(&world, &pending_id).await, "ended:rejected");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unsupported_activities_are_acknowledged_without_changing_any_relation() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
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
    let inbound_before = count(&world, "profile_inbound_activities").await;
    let jobs_before = count(&world, "profile_delivery_jobs").await;

    let note = json!({"id": peer.activity_id("note-1"), "type": "Note", "content": "hi"});
    let unsupported: Vec<Value> = vec![
        json!({"id": peer.activity_id("create-1"), "type": "Create", "actor": peer.actor_url, "object": note}),
        json!({"id": peer.activity_id("like-1"), "type": "Like", "actor": peer.actor_url, "object": profile.actor_url}),
        json!({"id": peer.activity_id("react-1"), "type": "EmojiReact", "actor": peer.actor_url, "object": profile.actor_url, "content": ":x:"}),
        json!({"id": peer.activity_id("announce-1"), "type": "Announce", "actor": peer.actor_url, "object": profile.actor_url}),
        json!({"id": peer.activity_id("delete-1"), "type": "Delete", "actor": peer.actor_url, "object": peer.actor_url}),
        json!({"id": peer.activity_id("block-1"), "type": "Block", "actor": peer.actor_url, "object": profile.actor_url}),
        peer.undo(
            &peer.activity_id("undo-block"),
            json!({"id": peer.activity_id("block-1"), "type": "Block", "actor": peer.actor_url, "object": profile.actor_url}),
        ),
        peer.undo(&peer.activity_id("undo-like"), json!({"id": peer.activity_id("like-1"), "type": "Like"})),
        peer.respond(
            "Accept",
            &peer.activity_id("accept-offer"),
            json!({"id": peer.activity_id("offer-1"), "type": "Offer"}),
        ),
    ];
    for activity in &unsupported {
        assert_eq!(
            peer.post(&world, &profile.nanoid, activity).await,
            ACCEPTED,
            "{activity}"
        );
    }
    assert_eq!(state_of(&world, &follow_id).await, "accepted");
    assert_eq!(count(&world, "profile_follows").await, 1);
    assert_eq!(
        count(&world, "profile_inbound_activities").await,
        inbound_before
    );
    assert_eq!(count(&world, "profile_delivery_jobs").await, jobs_before);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn malformed_references_are_refused_before_anything_is_stored() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();

    let mut cases = vec![
        // Follow without a usable object.
        json!({"id": peer.activity_id("f1"), "type": "Follow", "actor": peer.actor_url}),
        // Undo / Accept / Reject without an object.
        json!({"id": peer.activity_id("u1"), "type": "Undo", "actor": peer.actor_url}),
        json!({"id": peer.activity_id("a1"), "type": "Accept", "actor": peer.actor_url}),
        json!({"id": peer.activity_id("r1"), "type": "Reject", "actor": peer.actor_url, "object": 5}),
        // Embedded Follow without an id.
        peer.undo(
            &peer.activity_id("u2"),
            json!({"type": "Follow", "actor": peer.actor_url, "object": profile.actor_url}),
        ),
    ];
    cases.push(peer.respond(
        "Accept",
        &peer.activity_id("a2"),
        json!({"type": "Follow", "actor": profile.actor_url, "object": peer.actor_url}),
    ));
    for case in &cases {
        assert_eq!(
            peer.post(&world, &profile.nanoid, case).await,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{case}"
        );
    }
    // Invalid activity id.
    assert_eq!(
        peer.post(
            &world,
            &profile.nanoid,
            &peer.follow("not a url", &profile.actor_url)
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(count(&world, "profile_follows").await, 0);
    assert_eq!(count(&world, "profile_inbound_activities").await, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_failing_accept_reservation_rolls_the_relation_change_back() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let follow_id = peer.activity_id("follow-1");
    let body = peer.follow(&follow_id, &profile.actor_url);

    world
        .pool()
        .execute(
            r#"
            CREATE FUNCTION p3i_reject_jobs() RETURNS trigger AS $$
            BEGIN RAISE EXCEPTION 'delivery reservation failed'; END; $$ LANGUAGE plpgsql;
            CREATE TRIGGER p3i_reject_jobs BEFORE INSERT ON profile_delivery_jobs
              FOR EACH ROW EXECUTE FUNCTION p3i_reject_jobs();
            "#,
        )
        .await
        .unwrap();
    assert_eq!(
        peer.post(&world, &profile.nanoid, &body).await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        count(&world, "profile_follows").await,
        0,
        "no relation without the job"
    );
    assert_eq!(count(&world, "profile_delivery_jobs").await, 0);
    assert_eq!(
        count(&world, "profile_inbound_activities").await,
        0,
        "the dedupe record rolls back too, so the redelivery is processed again"
    );

    world
        .pool()
        .execute("DROP TRIGGER p3i_reject_jobs ON profile_delivery_jobs")
        .await
        .unwrap();
    assert_eq!(peer.post(&world, &profile.nanoid, &body).await, ACCEPTED);
    assert_eq!(state_of(&world, &follow_id).await, "accepted");
    assert_eq!(count(&world, "profile_delivery_jobs").await, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_failing_relation_commit_leaves_no_accept_job() {
    let world = GraphWorld::new().await;
    let profile = world.actor_profile().await;
    let peer = RemotePeer::new();
    let body = peer.follow(&peer.activity_id("follow-1"), &profile.actor_url);

    // The relation row is rejected only when the transaction commits, i.e. after the Accept job
    // has been inserted: both must disappear.
    world
        .pool()
        .execute(
            r#"
            CREATE FUNCTION p3i_reject_follows() RETURNS trigger AS $$
            BEGIN RAISE EXCEPTION 'relation change failed at commit'; END; $$ LANGUAGE plpgsql;
            CREATE CONSTRAINT TRIGGER p3i_reject_follows AFTER INSERT ON profile_follows
              DEFERRABLE INITIALLY DEFERRED
              FOR EACH ROW EXECUTE FUNCTION p3i_reject_follows();
            "#,
        )
        .await
        .unwrap();
    assert_eq!(
        peer.post(&world, &profile.nanoid, &body).await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(count(&world, "profile_follows").await, 0);
    assert_eq!(count(&world, "profile_delivery_jobs").await, 0);
    assert_eq!(count(&world, "profile_inbound_activities").await, 0);
}
