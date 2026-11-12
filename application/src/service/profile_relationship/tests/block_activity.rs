//! Individual actor block announces an AP `Block` (user decision) next to the Undo / Reject of the
//! detached Follows; the unblock takes it back with an `Undo(Block)` (nothing when the `Block` was
//! never sent). A domain block never sends a `Block` (`domain_block` tests). Same 120 s / lease rules
//! as the Follow's Undo.

use super::support::{Env, User, LOCAL_BASE};
use crate::service::domain_control::DomainControlUseCase;
use crate::service::profile_relationship::ProfileRelationshipUseCase;
use driver::http_signing::HttpSignatureVerifierImpl;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::http_signing::{
    HttpSignatureVerificationInput, HttpSignatureVerifier, SignatureVerificationResult,
};
use kernel::interfaces::repository::{
    DependOnProfileDeliveryJobRepository, ProfileDeliveryJobRepository,
};
use kernel::prelude::entity::{
    DeliveryActivityType, DeliveryClaimPolicy, DomainControlMode, ProfileDeliveryJob,
    ProfileDeliveryJobState,
};
use serde_json::Value;
use std::collections::HashMap;
use time::{Duration, OffsetDateTime};
use wiremock::ResponseTemplate;

fn body(job: &ProfileDeliveryJob) -> Value {
    serde_json::from_str(&job.payload).unwrap()
}

fn actor_url(user: &User) -> String {
    format!("{LOCAL_BASE}/ap/profiles/{}", user.nanoid)
}

fn of_type(jobs: &[ProfileDeliveryJob], t: DeliveryActivityType) -> Vec<&ProfileDeliveryJob> {
    jobs.iter().filter(|job| job.activity_type == t).collect()
}

async fn deliver_all(env: &Env) {
    env.respond_inbox(ResponseTemplate::new(202)).await;
    for _ in 0..8 {
        if env.cycle("w").await.attempted == 0 {
            break;
        }
    }
}

async fn block(env: &Env, user: &User, target: &str) -> String {
    env.module
        .block_actor(&user.auth, None, &user.nanoid, target)
        .await
        .unwrap()
        .nanoid
}

async fn unblock(env: &Env, user: &User, nanoid: &str) {
    env.module
        .unblock_actor(&user.auth, None, &user.nanoid, nanoid)
        .await
        .unwrap();
}

async fn claim_one(env: &Env) {
    claim_one_with_lease(env, 60).await;
}

async fn claim_one_with_lease(env: &Env, lease_seconds: i64) {
    let mut executor = env.module.database_connection().connection().await.unwrap();
    let claimed = env
        .module
        .profile_delivery_job_repository()
        .claim_due(
            &mut executor,
            "other-worker",
            OffsetDateTime::now_utc(),
            &DeliveryClaimPolicy {
                lease_duration: Duration::seconds(lease_seconds),
                limit: 1,
                max_attempts: 12,
            },
        )
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
}

async fn verify(
    env: &Env,
    request: &wiremock::Request,
    key_id: &str,
    public_key: &str,
) -> SignatureVerificationResult {
    let mut verifier = HttpSignatureVerifierImpl::new().unwrap();
    verifier.cache_actor_key(key_id, public_key.to_string());
    let headers: HashMap<String, String> = request
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                value.to_str().unwrap().to_string(),
            )
        })
        .collect();
    verifier
        .verify(&HttpSignatureVerificationInput {
            method: "POST".to_string(),
            url: env.peer_url("/inbox"),
            headers,
            body: Some(request.body.clone()),
        })
        .await
        .unwrap()
}

async fn types_received(env: &Env) -> Vec<String> {
    let mut types: Vec<String> = env
        .inbox_requests()
        .await
        .iter()
        .map(|request| {
            serde_json::from_slice::<Value>(&request.body).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    types.sort();
    types
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_block_delivers_a_signed_block_whose_object_is_the_blocked_actor() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let public_key = env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");

    block(&env, &alice, &bob).await;

    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(jobs.len(), 1, "no relation existed, only the Block");
    let job = &jobs[0];
    assert_eq!(job.activity_type, DeliveryActivityType::Block);
    assert_eq!(job.follow_id, None);
    assert_eq!(job.target_inbox.as_str(), env.peer_url("/inbox"));
    let value = body(job);
    assert_eq!(value["type"], "Block");
    assert_eq!(value["actor"], actor_url(&alice));
    assert_eq!(value["object"], bob);
    assert_eq!(value["to"][0], bob);
    assert!(
        value["id"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{}/blocks/", actor_url(&alice))),
        "{value}"
    );

    deliver_all(&env).await;
    let requests = env.inbox_requests().await;
    assert_eq!(requests.len(), 1);
    let key_id = format!("{LOCAL_BASE}/ap/profiles/{}#main-key", alice.nanoid);
    assert_eq!(
        verify(&env, &requests[0], &key_id, &public_key).await,
        SignatureVerificationResult::Valid { key_id }
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].body).unwrap(),
        value
    );

    // The same block again is a conflict and reserves nothing; the id is stable per block.
    assert!(env
        .module
        .block_actor(&alice.auth, None, &alice.nanoid, &bob)
        .await
        .is_err());
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unblocking_a_block_that_was_never_sent_cancels_it_and_sends_no_undo() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    let block_nanoid = block(&env, &alice, &bob).await;

    unblock(&env, &alice, &block_nanoid).await;

    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(jobs.len(), 1, "no Undo(Block)");
    assert_eq!(jobs[0].state, ProfileDeliveryJobState::Cancelled);
    deliver_all(&env).await;
    assert!(
        env.inbox_requests().await.is_empty(),
        "nothing reaches the peer"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unblocking_after_the_block_was_delivered_sends_an_undo_that_embeds_it() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let public_key = env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    let block_nanoid = block(&env, &alice, &bob).await;
    deliver_all(&env).await;
    let original = body(&env.jobs_of(&alice.profile_id).await[0]);

    unblock(&env, &alice, &block_nanoid).await;

    let jobs = env.jobs_of(&alice.profile_id).await;
    let undo = of_type(&jobs, DeliveryActivityType::Undo);
    assert_eq!(undo.len(), 1);
    let value = body(undo[0]);
    assert_eq!(value["type"], "Undo");
    assert_eq!(value["actor"], actor_url(&alice));
    assert_eq!(value["object"]["type"], "Block");
    assert_eq!(value["object"]["id"], original["id"]);
    assert_eq!(value["object"]["object"], bob);
    assert_eq!(value["object"]["actor"], actor_url(&alice));
    assert!(value["object"].get("@context").is_none());
    assert_eq!(undo[0].target_inbox, jobs[0].target_inbox);
    assert!(
        undo[0].next_attempt_at < OffsetDateTime::now_utc() + Duration::seconds(60),
        "a delivered Block needs no delay"
    );

    deliver_all(&env).await;
    let requests = env.inbox_requests().await;
    assert_eq!(types_received(&env).await, vec!["Undo".to_string()]);
    let key_id = format!("{LOCAL_BASE}/ap/profiles/{}#main-key", alice.nanoid);
    assert_eq!(
        verify(&env, &requests[0], &key_id, &public_key).await,
        SignatureVerificationResult::Valid { key_id }
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_undo_of_a_block_that_may_be_in_flight_waits_for_it() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    let block_nanoid = block(&env, &alice, &bob).await;
    claim_one(&env).await;

    unblock(&env, &alice, &block_nanoid).await;

    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(jobs[0].state, ProfileDeliveryJobState::Cancelled);
    let undo = of_type(&jobs, DeliveryActivityType::Undo);
    assert_eq!(undo.len(), 1);
    assert!(
        undo[0].next_attempt_at > OffsetDateTime::now_utc() + Duration::seconds(60),
        "the Undo(Block) is sent only after the in-flight Block could have landed"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_block_of_a_local_profile_delivers_nothing() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let bob = env.seed_user().await;
    let block_nanoid = block(&env, &alice, &actor_url(&bob)).await;
    unblock(&env, &alice, &block_nanoid).await;
    assert_eq!(
        env.count("SELECT count(*) FROM profile_delivery_jobs")
            .await,
        0
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_unreachable_peer_still_gets_the_block_through_a_known_inbox_and_otherwise_none() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    // alice follows bob (the Follow job knows bob's inbox), then bob stops answering actor requests.
    let follow = env
        .module
        .follow_profile(&alice.auth, None, &alice.nanoid, &bob)
        .await
        .unwrap();
    env.respond_inbox(ResponseTemplate::new(202)).await;

    let block_nanoid = block(&env, &alice, &bob).await;
    let jobs = env.jobs_of(&alice.profile_id).await;
    let blocks = of_type(&jobs, DeliveryActivityType::Block);
    assert_eq!(blocks.len(), 1, "the inbox of the cancelled Follow is used");
    assert_eq!(blocks[0].target_inbox.as_str(), env.peer_url("/inbox"));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM profile_follows WHERE nanoid = $1")
            .bind(&follow.nanoid)
            .fetch_one(&env.iso.pool)
            .await
            .unwrap(),
        "ended"
    );
    unblock(&env, &alice, &block_nanoid).await;

    // Without any known inbox the block still applies locally; only the Block is not reserved.
    let carol = format!("http://127.0.0.1:{}/users/carol", env.peer.address().port());
    let before = env.jobs_of(&alice.profile_id).await.len();
    block(&env, &alice, &carol).await;
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), before);
    assert_eq!(env.count("SELECT count(*) FROM profile_blocks").await, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_peer_failure_does_not_undo_the_block_and_the_block_is_retried() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    block(&env, &alice, &bob).await;

    env.respond_inbox(ResponseTemplate::new(503)).await;
    let report = env.cycle("w").await;
    assert_eq!((report.attempted, report.delivered), (1, 0));
    assert_eq!(env.count("SELECT count(*) FROM profile_blocks").await, 1);
    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(jobs[0].state, ProfileDeliveryJobState::Retrying);

    env.advance_clock(Duration::hours(1));
    env.respond_inbox(ResponseTemplate::new(202)).await;
    assert_eq!(env.cycle("w").await.delivered, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_failed_block_reservation_rolls_back_the_block_and_the_detached_relations() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    let follow = env
        .module
        .follow_profile(&alice.auth, None, &alice.nanoid, &bob)
        .await
        .unwrap();
    env.execute(
        "CREATE FUNCTION d8r_block_fail() RETURNS trigger AS $$ BEGIN RAISE EXCEPTION 'injected failure'; END $$ LANGUAGE plpgsql;
         CREATE TRIGGER d8r_block_fail BEFORE INSERT ON profile_delivery_jobs
           FOR EACH ROW WHEN (NEW.activity_type = 'Block') EXECUTE FUNCTION d8r_block_fail();",
    )
    .await;

    assert!(env
        .module
        .block_actor(&alice.auth, None, &alice.nanoid, &bob)
        .await
        .is_err());

    assert_eq!(env.count("SELECT count(*) FROM profile_blocks").await, 0);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM profile_follows WHERE nanoid = $1")
            .bind(&follow.nanoid)
            .fetch_one(&env.iso.pool)
            .await
            .unwrap(),
        "pending",
        "the detachment is rolled back with the block"
    );
    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].state, ProfileDeliveryJobState::Pending);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_domain_block_never_sends_a_block_even_to_an_actor_that_is_individually_blocked() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    let port = env.peer.address().port();
    let url = format!("http://localhost:{port}/users/bob");
    env.mount_actor_as("/users/bob", &url).await;
    // Follow bob (delivered), then block the whole domain: Undo(Follow) only.
    env.module
        .follow_profile(&alice.auth, None, &alice.nanoid, &url)
        .await
        .unwrap();
    deliver_all(&env).await;

    env.module
        .put_domain_control(
            &alice.auth,
            None,
            &alice.nanoid,
            "localhost",
            DomainControlMode::Block,
            None,
        )
        .await
        .unwrap();
    deliver_all(&env).await;

    assert_eq!(types_received(&env).await, vec!["Undo".to_string()]);
    let jobs = env.jobs_of(&alice.profile_id).await;
    assert!(of_type(&jobs, DeliveryActivityType::Block).is_empty());
    let undo = of_type(&jobs, DeliveryActivityType::Undo);
    assert_eq!(body(undo[0])["object"]["type"], "Follow");
    assert_eq!(
        env.count("SELECT count(*) FROM profile_delivery_jobs WHERE activity_type = 'Block'")
            .await,
        0
    );
}

/// A peer that keeps its block state per actor pair like Mastodon / Iceshrimp: a `Block` blocks, any
/// `Undo(Block)` unblocks (it does not look at which Block it names). Requests answered with a failure
/// status are logged but change nothing.
#[derive(Default)]
struct PeerState {
    blocked: bool,
    log: Vec<String>,
}

type SharedPeer = std::sync::Arc<std::sync::Mutex<PeerState>>;

async fn mount_stateful_inbox(
    env: &Env,
    status: std::sync::Arc<std::sync::atomic::AtomicU16>,
) -> SharedPeer {
    let state: SharedPeer = SharedPeer::default();
    mount_stateful_inbox_into(env, status, state.clone()).await;
    state
}

/// Mounts the stateful inbox on top of an existing peer state (used after `peer.reset()`, which also
/// removes the actor document: the actor can no longer be resolved).
async fn mount_stateful_inbox_into(
    env: &Env,
    status: std::sync::Arc<std::sync::atomic::AtomicU16>,
    state: SharedPeer,
) {
    let shared = state;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/inbox"))
        .respond_with(move |request: &wiremock::Request| {
            let value: Value = serde_json::from_slice(&request.body).unwrap();
            let label = match (value["type"].as_str(), value["object"]["type"].as_str()) {
                (Some("Undo"), Some("Block")) => "Undo(Block)",
                (Some("Block"), _) => "Block",
                _ => "other",
            };
            let code = status.load(std::sync::atomic::Ordering::SeqCst);
            let mut state = shared.lock().unwrap();
            if (200..300).contains(&code) {
                match label {
                    "Block" => state.blocked = true,
                    "Undo(Block)" => state.blocked = false,
                    _ => {}
                }
                state.log.push(label.to_string());
            } else {
                state.log.push(format!("{code}:{label}"));
            }
            ResponseTemplate::new(code)
        })
        .mount(&env.peer)
        .await;
}

fn status(code: u16) -> std::sync::Arc<std::sync::atomic::AtomicU16> {
    std::sync::Arc::new(std::sync::atomic::AtomicU16::new(code))
}

async fn run_worker(env: &Env) {
    for _ in 0..6 {
        if env.cycle("w").await.attempted == 0 {
            break;
        }
    }
}

fn log(peer: &SharedPeer) -> Vec<String> {
    peer.lock().unwrap().log.clone()
}

fn blocked(peer: &SharedPeer) -> bool {
    peer.lock().unwrap().blocked
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_undo_waiting_for_a_retry_cannot_unblock_a_later_reblock() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    let code = status(202);
    let peer = mount_stateful_inbox(&env, code.clone()).await;

    // B1 reaches the peer; the unblock's Undo(B1) then fails once and waits for a retry.
    let first = block(&env, &alice, &bob).await;
    run_worker(&env).await;
    unblock(&env, &alice, &first).await;
    code.store(503, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(env.cycle("w").await.attempted, 1);
    let undo_b1 = of_type(
        &env.jobs_of(&alice.profile_id).await,
        DeliveryActivityType::Undo,
    )[0]
    .clone();
    assert_eq!(undo_b1.state, ProfileDeliveryJobState::Retrying);

    // The reblock cancels the waiting Undo and sends B2 without delay (nothing is in flight).
    block(&env, &alice, &bob).await;
    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(
        env.job(&undo_b1.id).await.state,
        ProfileDeliveryJobState::Cancelled
    );
    let block_jobs = of_type(&jobs, DeliveryActivityType::Block);
    assert_eq!(block_jobs.len(), 2);
    assert!(block_jobs[1].next_attempt_at < OffsetDateTime::now_utc() + Duration::seconds(60));

    code.store(202, std::sync::atomic::Ordering::SeqCst);
    run_worker(&env).await;
    env.advance_clock(Duration::hours(3));
    run_worker(&env).await;

    assert_eq!(
        log(&peer),
        vec!["Block", "503:Undo(Block)", "Block"],
        "the failed Undo(B1) is never retried after the reblock"
    );
    assert!(blocked(&peer), "the latest block stands at the peer");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_undo_in_flight_at_the_reblock_is_waited_for_before_the_new_block_is_sent() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    let peer = mount_stateful_inbox(&env, status(202)).await;

    let first = block(&env, &alice, &bob).await;
    run_worker(&env).await;
    unblock(&env, &alice, &first).await;
    // A worker holds the lease of Undo(B1): it may be landing at the peer right now.
    claim_one(&env).await;
    let undo_b1 = of_type(
        &env.jobs_of(&alice.profile_id).await,
        DeliveryActivityType::Undo,
    )[0]
    .clone();
    assert_eq!(undo_b1.state, ProfileDeliveryJobState::InFlight);
    let lease_end = undo_b1.lease_expires_at.expect("in flight has a lease");

    block(&env, &alice, &bob).await;

    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(
        env.job(&undo_b1.id).await.state,
        ProfileDeliveryJobState::Cancelled
    );
    let b2 = of_type(&jobs, DeliveryActivityType::Block)[1].clone();
    assert!(
        b2.next_attempt_at >= lease_end,
        "B2 starts after the lease of the in-flight Undo ({lease_end}), not at {}",
        b2.next_attempt_at
    );
    assert!(b2.next_attempt_at > OffsetDateTime::now_utc() + Duration::seconds(60));

    // Now: B2 is not due, so the worker sends nothing; the in-flight Undo lands (simulated at the peer)
    // and only after that the new Block follows.
    assert_eq!(env.cycle("w").await.attempted, 0);
    {
        let mut state = peer.lock().unwrap();
        state.blocked = false;
        state.log.push("Undo(Block)-late".to_string());
    }
    env.advance_clock(Duration::seconds(200));
    run_worker(&env).await;
    assert_eq!(log(&peer), vec!["Block", "Undo(Block)-late", "Block"]);
    assert!(blocked(&peer), "the new Block is the last word");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_unsent_undo_is_cancelled_by_the_reblock_and_the_reblock_is_not_delayed() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    let peer = mount_stateful_inbox(&env, status(202)).await;
    let first = block(&env, &alice, &bob).await;
    run_worker(&env).await;
    unblock(&env, &alice, &first).await;

    block(&env, &alice, &bob).await;
    run_worker(&env).await;

    assert_eq!(
        log(&peer),
        vec!["Block", "Block"],
        "no Undo reaches the peer"
    );
    assert!(blocked(&peer));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unblocking_again_before_the_reblock_was_sent_still_undoes_the_block_the_peer_holds() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    let peer = mount_stateful_inbox(&env, status(202)).await;
    let first = block(&env, &alice, &bob).await;
    run_worker(&env).await;
    // unblock (Undo(B1) waits), reblock (Undo(B1) cancelled, B2 waits), unblock again before B2 is sent.
    unblock(&env, &alice, &first).await;
    let second = block(&env, &alice, &bob).await;
    unblock(&env, &alice, &second).await;

    let jobs = env.jobs_of(&alice.profile_id).await;
    let live_undo: Vec<_> = of_type(&jobs, DeliveryActivityType::Undo)
        .into_iter()
        .filter(|job| job.state == ProfileDeliveryJobState::Pending)
        .collect();
    assert_eq!(live_undo.len(), 1, "exactly one Undo is left to send");
    assert_eq!(
        body(live_undo[0])["object"]["id"],
        body(of_type(&jobs, DeliveryActivityType::Block)[0])["id"],
        "it undoes the Block the peer actually holds (B1: B2 was never sent)"
    );

    run_worker(&env).await;
    assert_eq!(log(&peer), vec!["Block", "Undo(Block)"]);
    assert!(!blocked(&peer), "the final state at the peer is unblocked");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_peer_outage_at_the_reblock_still_cancels_the_retrying_undo_and_the_block_goes_to_the_known_inbox(
) {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    let code = status(202);
    let peer = mount_stateful_inbox(&env, code.clone()).await;
    let first = block(&env, &alice, &bob).await;
    run_worker(&env).await;
    unblock(&env, &alice, &first).await;
    code.store(503, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(env.cycle("w").await.attempted, 1);
    let undo_b1 = of_type(
        &env.jobs_of(&alice.profile_id).await,
        DeliveryActivityType::Undo,
    )[0]
    .clone();
    assert_eq!(undo_b1.state, ProfileDeliveryJobState::Retrying);

    // The peer goes away: its actor document cannot be fetched any more (only the inbox answers again
    // once it recovers). No follow is detached by this block, so only the pair's own jobs know the inbox.
    env.peer.reset().await;
    code.store(202, std::sync::atomic::Ordering::SeqCst);
    mount_stateful_inbox_into(&env, code.clone(), peer.clone()).await;
    block(&env, &alice, &bob).await;

    assert_eq!(
        env.job(&undo_b1.id).await.state,
        ProfileDeliveryJobState::Cancelled,
        "the old Undo is cancelled although the actor could not be resolved"
    );
    let jobs = env.jobs_of(&alice.profile_id).await;
    let blocks = of_type(&jobs, DeliveryActivityType::Block);
    assert_eq!(
        blocks.len(),
        2,
        "the new Block is reserved for the known inbox"
    );
    assert_eq!(blocks[1].target_inbox, blocks[0].target_inbox);

    run_worker(&env).await;
    env.advance_clock(Duration::hours(3));
    run_worker(&env).await;
    assert_eq!(log(&peer), vec!["Block", "503:Undo(Block)", "Block"]);
    assert!(blocked(&peer), "local blocked and the peer blocked");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_peer_outage_at_the_reblock_waits_for_an_in_flight_undo_before_the_new_block() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    let code = status(202);
    let peer = mount_stateful_inbox(&env, code.clone()).await;
    let first = block(&env, &alice, &bob).await;
    run_worker(&env).await;
    unblock(&env, &alice, &first).await;
    claim_one(&env).await;
    let undo_b1 = of_type(
        &env.jobs_of(&alice.profile_id).await,
        DeliveryActivityType::Undo,
    )[0]
    .clone();
    let lease_end = undo_b1.lease_expires_at.expect("in flight has a lease");

    env.peer.reset().await;
    mount_stateful_inbox_into(&env, code.clone(), peer.clone()).await;
    block(&env, &alice, &bob).await;

    assert_eq!(
        env.job(&undo_b1.id).await.state,
        ProfileDeliveryJobState::Cancelled
    );
    let b2 = of_type(
        &env.jobs_of(&alice.profile_id).await,
        DeliveryActivityType::Block,
    )[1]
    .clone();
    assert!(
        b2.next_attempt_at >= lease_end,
        "B2 waits for the Undo that may be landing"
    );

    assert_eq!(env.cycle("w").await.attempted, 0);
    {
        let mut state = peer.lock().unwrap();
        state.blocked = false;
        state.log.push("Undo(Block)-late".to_string());
    }
    env.advance_clock(Duration::seconds(200));
    run_worker(&env).await;
    assert_eq!(log(&peer), vec!["Block", "Undo(Block)-late", "Block"]);
    assert!(blocked(&peer), "the new Block is the last word");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn the_lease_bound_survives_a_chain_of_unblock_reblock_unblock() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    let first = block(&env, &alice, &bob).await;
    // A worker holds B1 with a long lease (600 s): it may land until then.
    claim_one_with_lease(&env, 600).await;
    let lease_end = env.jobs_of(&alice.profile_id).await[0]
        .lease_expires_at
        .expect("in flight has a lease");

    unblock(&env, &alice, &first).await;
    let undo_1 = of_type(
        &env.jobs_of(&alice.profile_id).await,
        DeliveryActivityType::Undo,
    )[0]
    .clone();
    assert!(
        undo_1.next_attempt_at >= lease_end,
        "the first Undo waits for B1"
    );

    // The reblock cancels the (unsent) Undo; the new Block is not sent yet; unblock again.
    let second = block(&env, &alice, &bob).await;
    assert_eq!(
        env.job(&undo_1.id).await.state,
        ProfileDeliveryJobState::Cancelled
    );
    unblock(&env, &alice, &second).await;

    let jobs = env.jobs_of(&alice.profile_id).await;
    let live: Vec<_> = of_type(&jobs, DeliveryActivityType::Undo)
        .into_iter()
        .filter(|job| job.state == ProfileDeliveryJobState::Pending)
        .collect();
    assert_eq!(live.len(), 1);
    assert_eq!(
        body(live[0])["object"]["id"],
        body(of_type(&jobs, DeliveryActivityType::Block)[0])["id"]
    );
    assert!(
        live[0].next_attempt_at >= lease_end,
        "the last Undo still waits for the lease of B1 ({lease_end}), not {}",
        live[0].next_attempt_at
    );
}

/// Holds the row lock of one delivery job in an open transaction (a worker that is renewing its lease
/// or recording a result), optionally with an uncommitted lease extension, so that a cancel of that job
/// queues behind it.
async fn hold_job_row(
    env: &Env,
    job_id: i64,
    extend_lease_to: Option<OffsetDateTime>,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = env.iso.pool.begin().await.unwrap();
    match extend_lease_to {
        Some(lease) => {
            sqlx::query("UPDATE profile_delivery_jobs SET lease_expires_at = $2 WHERE id = $1")
                .bind(job_id)
                .bind(lease)
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        None => {
            sqlx::query("SELECT id FROM profile_delivery_jobs WHERE id = $1 FOR UPDATE")
                .bind(job_id)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        }
    }
    tx
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_lease_extended_after_the_read_still_bounds_the_undo() {
    let env = std::sync::Arc::new(Env::new().await);
    let alice = env.seed_user().await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    let first = block(&env, &alice, &bob).await;
    claim_one(&env).await;
    let job_id = *env.jobs_of(&alice.profile_id).await[0].id.as_ref();

    // The worker renews the lease to +900 s (uncommitted: the unblock reads the old value first).
    let renewed = super::support::round(OffsetDateTime::now_utc() + Duration::seconds(900));
    let tx = hold_job_row(&env, job_id, Some(renewed)).await;
    let mut unblocking = tokio::spawn({
        let env = env.clone();
        let auth = alice.auth.clone();
        let nanoid = alice.nanoid.clone();
        async move {
            env.module
                .unblock_actor(&auth, None, &nanoid, &first)
                .await
                .unwrap();
        }
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(1000), &mut unblocking)
            .await
            .is_err(),
        "the unblock waits for the worker's row"
    );
    tx.commit().await.unwrap();
    unblocking.await.unwrap();

    let jobs = env.jobs_of(&alice.profile_id).await;
    let undo = of_type(&jobs, DeliveryActivityType::Undo)[0];
    assert!(
        undo.next_attempt_at >= renewed,
        "the Undo starts after the renewed lease ({renewed}), not at {}",
        undo.next_attempt_at
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_slow_cancel_keeps_its_bound_through_a_chain_of_operations() {
    let env = std::sync::Arc::new(Env::new().await);
    let alice = env.seed_user().await;
    env.mount_actor("/users/bob").await;
    let bob = env.peer_url("/users/bob");
    let first = block(&env, &alice, &bob).await;
    // The lease (60 s) is far below the 120 s grace: the grace counted from after the cancel dominates.
    claim_one(&env).await;
    let job_id = *env.jobs_of(&alice.profile_id).await[0].id.as_ref();

    let tx = hold_job_row(&env, job_id, None).await;
    let mut unblocking = tokio::spawn({
        let env = env.clone();
        let auth = alice.auth.clone();
        let nanoid = alice.nanoid.clone();
        async move {
            env.module
                .unblock_actor(&auth, None, &nanoid, &first)
                .await
                .unwrap();
        }
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(2000), &mut unblocking)
            .await
            .is_err(),
        "the cancel waits two seconds for the worker's row"
    );
    let released_at = OffsetDateTime::now_utc();
    tx.commit().await.unwrap();
    unblocking.await.unwrap();

    let undo_1 = of_type(
        &env.jobs_of(&alice.profile_id).await,
        DeliveryActivityType::Undo,
    )[0]
    .clone();
    assert!(
        undo_1.next_attempt_at >= released_at + Duration::seconds(120),
        "the first Undo counts the grace from after the slow cancel"
    );

    // reblock (cancels the unsent Undo) and unblock again: the last Undo is never earlier than the first.
    let second = block(&env, &alice, &bob).await;
    unblock(&env, &alice, &second).await;
    let jobs = env.jobs_of(&alice.profile_id).await;
    let live: Vec<_> = of_type(&jobs, DeliveryActivityType::Undo)
        .into_iter()
        .filter(|job| job.state == ProfileDeliveryJobState::Pending)
        .collect();
    assert_eq!(live.len(), 1);
    assert!(
        live[0].next_attempt_at >= undo_1.next_attempt_at,
        "the chain must not move the Undo earlier ({} < {})",
        live[0].next_attempt_at,
        undo_1.next_attempt_at
    );
}
