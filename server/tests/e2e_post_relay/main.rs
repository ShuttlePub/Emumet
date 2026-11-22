//! T1: P1 post-relay end-to-end suite (plan 8 T1 / 11.1 / 11.2). Real Kratos, Hydra, Keto and
//! PostgreSQL (e2e/post-relay/compose.yml), the source-built server and mock P1 receiver.
//! Run: bash e2e/post-relay/run.sh   (starts the stack, builds the release image, runs this suite)
#![allow(dead_code)]
mod alarms;
mod alerts;
mod capacity;
mod db;
mod delivery;
mod emumet;
mod intake;
mod lifecycle;
mod ory;
mod ory_outage;
mod receiver;
mod receiver_p1;
mod stack;
mod supplier;
mod races;
mod retention;
mod worker;
mod world;

use receiver::Behavior;
use serde_json::json;
use std::time::Duration;
use stack::PUBLIC;
use world::*;

const NEEDS_STACK: &str = "needs the T1 stack: bash e2e/post-relay/run.sh";

/// Case 1 (and 22/26): real authentication -> Profile -> link -> signed inbox -> relay, the
/// receiver verifies without any link state, and nothing per-delivery remains afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case01_real_auth_profile_link_signed_inbox_to_relay() {
    let w = World::setup().await;
    let a = w.linked("c01", 0).await;
    let (status, me) = w.server.api("GET", "/api/v1/me", &a.user.jwt, None).await;
    assert_eq!(status, 200, "{me}");
    let links = w.server.links(&a.user.jwt, &a.profile).await;
    assert_eq!(links["items"][0]["state"], "active");
    assert_eq!(links["items"][0]["generation"], 1);

    let activity = w.peer.note("hello relay");
    let response = w.peer.deliver_json(&w.server, &a.profile, &activity).await;
    assert_eq!(response.status(), 202, "{}", response.text().await.unwrap());
    let accepted = w.receiver.wait_accepted(1, Duration::from_secs(30)).await;
    let e = &accepted[0].envelope;
    assert_eq!(e["recipient"]["link_id"], a.link_id);
    assert_eq!(e["recipient"]["link_generation"], 1);
    assert_eq!(e["sender"]["instance_actor"], format!("{PUBLIC}/ap/instance"));
    assert_eq!(e["verification"]["result"], "verified");
    assert_eq!(accepted[0].raw_body, serde_json::to_vec(&activity).unwrap());
    assert!(w.receiver.instance_fetches() >= 1, "instance key must be fetched over HTTP");
    let sig = &accepted[0].headers["signature"];
    assert!(sig.contains(&format!("keyId=\"{PUBLIC}/ap/instance#key-")), "{sig}");

    // Case 22: success leaves no job / attempts / body / per-delivery terminal row.
    w.receipts_gone().await;
    for table in ["post_relay_jobs", "post_relay_attempts", "post_relay_bodies", "post_relay_dead_letters", "post_relay_audits"] {
        assert_eq!(w.db.count(table).await, 0, "{table}");
    }
    assert_eq!(w.db.i64("SELECT succeeded FROM post_relay_counters").await, 1);
    let _ = (Behavior::Accept, json!({}), NEEDS_STACK);
}
