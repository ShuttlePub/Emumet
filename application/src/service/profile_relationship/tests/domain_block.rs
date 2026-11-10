//! D8R: a domain `block` write detaches the existing relations with that exact domain, in the same
//! transaction, with the minimal clean-up activities (`Undo(Follow)` for a Follow of ours,
//! `Reject(Follow)` for a Follow of theirs) and never an ActivityPub `Block`.
//!
//! Remote peers are the wiremock server reached through the name `localhost` (see `domain_guard`):
//! a domain control cannot be stored for an IP literal, so the actor IDs use `localhost` while the
//! peer's inbox stays `http://127.0.0.1:…`. Wire-level checks read what the signed delivery worker
//! really POSTed to the peer.

use super::support::{Env, User, LOCAL_BASE};
use crate::service::domain_control::DomainControlUseCase;
use crate::service::domain_follow_guard::lock_profile_for_domain_control_write;
use crate::service::profile_relationship::ProfileRelationshipUseCase;
use driver::http_signing::HttpSignatureVerifierImpl;
use kernel::interfaces::database::{DependOnTransactionManager, TransactionManager};
use kernel::interfaces::http_signing::{
    HttpSignatureVerificationInput, HttpSignatureVerifier, SignatureVerificationResult,
};
use kernel::interfaces::repository::{
    DependOnProfileDeliveryEnqueuer, DependOnProfileDeliveryJobRepository,
    DependOnProfileFollowRepository, ProfileDeliveryEnqueuer, ProfileDeliveryJobRepository,
    ProfileFollowRepository,
};
use kernel::prelude::entity::{
    DeliveryActivityType, DeliveryClaimPolicy, DomainControlMode, GraphActivityId, GraphActorUrl,
    GraphInboxUrl, NewProfileDeliveryJob, ProfileDeliveryJob, ProfileDeliveryJobState,
    ProfileFollow, ProfileFollowId, ProfileGraphActor,
};
use kernel::KernelError;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration as StdDuration;
use time::{Duration, OffsetDateTime};
use wiremock::ResponseTemplate;

const BLOCKED_HOST: &str = "localhost";

fn alice_url(user: &User) -> String {
    format!("{LOCAL_BASE}/ap/profiles/{}", user.nanoid)
}

fn body(job: &ProfileDeliveryJob) -> Value {
    serde_json::from_str(&job.payload).unwrap()
}

/// An actor of the peer reached through the host name `localhost`.
async fn localhost_actor(env: &Env, path: &str) -> String {
    let url = format!("http://localhost:{}{path}", env.peer.address().port());
    env.mount_actor_as(path, &url).await;
    url
}

async fn follow(env: &Env, user: &User, nanoid: &str, target: &str) -> String {
    env.module
        .follow_profile(&user.auth, None, nanoid, target)
        .await
        .unwrap()
        .nanoid
}

async fn block_domain(
    env: &Env,
    user: &User,
    domain: &str,
) -> error_stack::Result<u64, KernelError> {
    env.module
        .put_domain_control(
            &user.auth,
            None,
            &user.nanoid,
            domain,
            DomainControlMode::Block,
            None,
        )
        .await
        .map(|dto| dto.revision)
}

async fn follow_state(env: &Env, nanoid: &str) -> (String, Option<String>) {
    sqlx::query_as("SELECT state, end_reason FROM profile_follows WHERE nanoid = $1")
        .bind(nanoid)
        .fetch_one(&env.iso.pool)
        .await
        .unwrap()
}

async fn follow_state_by_id(env: &Env, id: &ProfileFollowId) -> (String, Option<String>) {
    sqlx::query_as("SELECT state, end_reason FROM profile_follows WHERE id = $1")
        .bind(id.as_ref())
        .fetch_one(&env.iso.pool)
        .await
        .unwrap()
}

async fn activity_id_of(env: &Env, nanoid: &str) -> String {
    sqlx::query_scalar("SELECT activity_id FROM profile_follows WHERE nanoid = $1")
        .bind(nanoid)
        .fetch_one(&env.iso.pool)
        .await
        .unwrap()
}

async fn follow_id_of(env: &Env, nanoid: &str) -> ProfileFollowId {
    let id: i64 = sqlx::query_scalar("SELECT id FROM profile_follows WHERE nanoid = $1")
        .bind(nanoid)
        .fetch_one(&env.iso.pool)
        .await
        .unwrap();
    ProfileFollowId::new(id)
}

fn ended_blocked() -> (String, Option<String>) {
    ("ended".to_string(), Some("blocked".to_string()))
}

async fn accept(env: &Env, id: &ProfileFollowId) {
    let deps = env.module.clone();
    let id = id.clone();
    env.module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                deps.profile_follow_repository()
                    .accept(executor, &id, OffsetDateTime::now_utc())
                    .await
                    .map(|_| ())
            })
        })
        .await
        .unwrap();
}

/// Every due job is delivered with a 202 (the worker is run until it has nothing left).
/// The peer's recorded requests are reset first, so `env.inbox_requests()` afterwards holds exactly
/// what this call delivered.
async fn deliver_all(env: &Env) {
    env.respond_inbox(ResponseTemplate::new(202)).await;
    for _ in 0..8 {
        if env.cycle("w").await.attempted == 0 {
            break;
        }
    }
}

/// A Follow of `alice` sent by `follower_url` (a peer actor) that she auto-accepted: an accepted
/// generation plus the Accept job P3i reserved with the peer inbox. `accept_due` decides whether the
/// Accept can be claimed by the worker right now.
async fn seed_incoming_follow(
    env: &Env,
    alice: &User,
    follower_url: &str,
    follow_activity: &str,
    accept_due: bool,
) -> ProfileFollow {
    let incoming = ProfileFollow::new_accepted(
        ProfileFollowId::new(kernel::generate_id()),
        ProfileGraphActor::Remote(GraphActorUrl::new(follower_url).unwrap()),
        ProfileGraphActor::Local(alice.profile_id.clone()),
        GraphActivityId::new(follow_activity).unwrap(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let not_before = if accept_due {
        OffsetDateTime::now_utc()
    } else {
        OffsetDateTime::now_utc() + Duration::hours(1)
    };
    let accept_job = NewProfileDeliveryJob::new(
        alice.profile_id.clone(),
        DeliveryActivityType::Accept,
        GraphActivityId::new(format!(
            "{}/activities/{}/accept",
            alice_url(alice),
            incoming.id().as_ref()
        ))
        .unwrap(),
        GraphInboxUrl::new(env.peer_url("/inbox")).unwrap(),
        format!(
            "{{\"type\":\"Accept\",\"object\":\"{}\"}}",
            incoming.activity_id().as_str()
        ),
        not_before,
    )
    .unwrap()
    .with_follow(incoming.id().clone());
    let deps = env.module.clone();
    let seeded = incoming.clone();
    env.module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                deps.profile_follow_repository()
                    .begin_generation(executor, &seeded)
                    .await?;
                deps.profile_delivery_enqueuer()
                    .enqueue(executor, &accept_job)
                    .await
                    .map(|_| ())
            })
        })
        .await
        .unwrap();
    incoming
}

/// A relation stored without any delivery job (a remote actor that is not in this test's peer).
async fn seed_bare_remote_follow(
    env: &Env,
    alice: &User,
    remote_actor: &str,
    remote_is_follower: bool,
) -> ProfileFollow {
    let remote = ProfileGraphActor::Remote(GraphActorUrl::new(remote_actor).unwrap());
    let me = ProfileGraphActor::Local(alice.profile_id.clone());
    let (follower, followee) = if remote_is_follower {
        (remote, me)
    } else {
        (me, remote)
    };
    let follow = ProfileFollow::new_accepted(
        ProfileFollowId::new(kernel::generate_id()),
        follower,
        followee,
        GraphActivityId::new(format!(
            "https://sub.localhost/follows/{}",
            kernel::generate_id()
        ))
        .unwrap(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let deps = env.module.clone();
    let seeded = follow.clone();
    env.module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                deps.profile_follow_repository()
                    .begin_generation(executor, &seeded)
                    .await
                    .map(|_| ())
            })
        })
        .await
        .unwrap();
    follow
}

/// Claims the next due job as a worker that is now sending it (lease held).
async fn claim_one(env: &Env, worker: &str) {
    use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
    let mut executor = env.module.database_connection().connection().await.unwrap();
    let claimed = env
        .module
        .profile_delivery_job_repository()
        .claim_due(
            &mut executor,
            worker,
            OffsetDateTime::now_utc(),
            &DeliveryClaimPolicy {
                lease_duration: Duration::seconds(60),
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

fn jobs_of_type(
    jobs: &[ProfileDeliveryJob],
    activity_type: DeliveryActivityType,
) -> Vec<&ProfileDeliveryJob> {
    jobs.iter()
        .filter(|job| job.activity_type == activity_type)
        .collect()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_domain_block_detaches_pending_and_accepted_relations_in_both_directions() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let public_key = env.create_key(&alice.profile_id, &alice.nanoid).await;
    let port = env.peer.address().port();
    let bob = localhost_actor(&env, "/users/bob").await;
    let carol = localhost_actor(&env, "/users/carol").await;
    let dave = format!("http://localhost:{port}/users/dave");
    let elsewhere = env.mount_actor("/users/eve").await;

    // A Profile of another Account must not be touched.
    let stranger = env.seed_user().await;
    env.create_key(&stranger.profile_id, &stranger.nanoid).await;

    let outgoing_accepted = follow(&env, &alice, &alice.nanoid, &bob).await;
    let outgoing_pending = follow(&env, &alice, &alice.nanoid, &carol).await;
    let other_host = follow(&env, &alice, &alice.nanoid, &elsewhere).await;
    let stranger_follow = follow(&env, &stranger, &stranger.nanoid, &bob).await;
    deliver_all(&env).await;
    accept(&env, &follow_id_of(&env, &outgoing_accepted).await).await;
    let incoming_activity = format!("http://localhost:{port}/follows/9");
    let incoming = seed_incoming_follow(&env, &alice, &dave, &incoming_activity, false).await;
    let parent_domain_follower =
        seed_bare_remote_follow(&env, &alice, "https://sub.localhost/users/x", true).await;
    let bob_activity = activity_id_of(&env, &outgoing_accepted).await;
    let carol_activity = activity_id_of(&env, &outgoing_pending).await;

    let revision = block_domain(&env, &alice, BLOCKED_HOST).await.unwrap();
    assert_eq!(revision, 1);

    // Relations: both directions, pending included, of exactly that host; nothing else.
    assert_eq!(
        follow_state(&env, &outgoing_accepted).await,
        ended_blocked()
    );
    assert_eq!(follow_state(&env, &outgoing_pending).await, ended_blocked());
    assert_eq!(
        follow_state_by_id(&env, incoming.id()).await,
        ended_blocked()
    );
    assert_eq!(follow_state(&env, &other_host).await.0, "pending");
    assert_eq!(
        follow_state_by_id(&env, parent_domain_follower.id())
            .await
            .0,
        "accepted",
        "sub.localhost is not covered by a block of localhost"
    );
    assert_eq!(follow_state(&env, &stranger_follow).await.0, "pending");

    // Reserved jobs: Undo for our two Follows (each referencing its own Follow), a Reject for
    // theirs, the unsent Accept cancelled. Never a Block.
    let jobs = env.jobs_of(&alice.profile_id).await;
    let undo = jobs_of_type(&jobs, DeliveryActivityType::Undo);
    let reject = jobs_of_type(&jobs, DeliveryActivityType::Reject);
    assert_eq!((undo.len(), reject.len()), (2, 1));
    let mut undone: Vec<String> = undo
        .iter()
        .map(|job| {
            let value = body(job);
            assert_eq!(value["type"], "Undo");
            assert_eq!(value["actor"], alice_url(&alice));
            assert_eq!(value["object"]["type"], "Follow");
            assert_eq!(
                value["object"]["actor"],
                alice_url(&alice),
                "the embedded Follow is ours"
            );
            value["object"]["id"].as_str().unwrap().to_string()
        })
        .collect();
    undone.sort();
    let mut expected = vec![bob_activity.clone(), carol_activity.clone()];
    expected.sort();
    assert_eq!(undone, expected);
    let reject = body(reject[0]);
    assert_eq!(reject["type"], "Reject");
    assert_eq!(reject["actor"], alice_url(&alice));
    assert_eq!(reject["object"]["type"], "Follow");
    assert_eq!(reject["object"]["id"], incoming_activity);
    assert_eq!(reject["object"]["actor"], dave);
    assert_eq!(reject["object"]["object"], alice_url(&alice));
    assert_eq!(reject["to"][0], dave);
    for job in &jobs {
        assert_ne!(body(job)["type"], "Block");
        if job.activity_type == DeliveryActivityType::Undo {
            assert_ne!(
                body(job)["object"]["id"],
                incoming_activity,
                "their Follow is never undone as if it were ours"
            );
        }
    }
    let accepts = jobs_of_type(&jobs, DeliveryActivityType::Accept);
    assert_eq!(accepts.len(), 1);
    assert_eq!(accepts[0].state, ProfileDeliveryJobState::Cancelled);
    // The other Account's Follow job is untouched.
    let others = env.jobs_of(&stranger.profile_id).await;
    assert!(others
        .iter()
        .all(|job| job.activity_type == DeliveryActivityType::Follow
            && job.state == ProfileDeliveryJobState::Delivered));

    // Wire level: what the signed worker really POSTs to the peer is exactly two Undo and one Reject.
    deliver_all(&env).await;
    let requests = env.inbox_requests().await;
    let key_id = format!("{LOCAL_BASE}/ap/profiles/{}#main-key", alice.nanoid);
    let mut types = Vec::new();
    for request in &requests {
        assert_eq!(
            verify(&env, request, &key_id, &public_key).await,
            SignatureVerificationResult::Valid {
                key_id: key_id.clone()
            }
        );
        let value: Value = serde_json::from_slice(&request.body).unwrap();
        types.push(value["type"].as_str().unwrap().to_string());
    }
    types.sort();
    assert_eq!(types, vec!["Reject", "Undo", "Undo"]);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_unsent_follow_is_cancelled_and_an_in_flight_one_gets_a_delayed_undo() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let bob = localhost_actor(&env, "/users/bob").await;
    let carol = localhost_actor(&env, "/users/carol").await;
    let sending = follow(&env, &alice, &alice.nanoid, &bob).await;
    // A worker holds the lease of bob's Follow: it may be landing on the peer right now.
    claim_one(&env, "other-worker").await;
    let unsent = follow(&env, &alice, &alice.nanoid, &carol).await;

    block_domain(&env, &alice, BLOCKED_HOST).await.unwrap();

    assert_eq!(follow_state(&env, &sending).await, ended_blocked());
    assert_eq!(follow_state(&env, &unsent).await, ended_blocked());
    let jobs = env.jobs_of(&alice.profile_id).await;
    let follows = jobs_of_type(&jobs, DeliveryActivityType::Follow);
    assert!(follows
        .iter()
        .all(|job| job.state == ProfileDeliveryJobState::Cancelled));
    let undo = jobs_of_type(&jobs, DeliveryActivityType::Undo);
    assert_eq!(
        undo.len(),
        1,
        "only the Follow that may have been sent is undone"
    );
    assert_eq!(
        body(undo[0])["object"]["id"],
        activity_id_of(&env, &sending).await
    );
    assert!(
        undo[0].next_attempt_at > OffsetDateTime::now_utc() + Duration::seconds(60),
        "the Undo waits until the in-flight Follow could have landed"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_in_flight_accept_delays_the_reject_so_that_it_cannot_arrive_first() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let dave = localhost_actor(&env, "/users/dave").await;
    let activity = env.peer_url("/follows/1");
    let sending = seed_incoming_follow(&env, &alice, &dave, &activity, true).await;
    claim_one(&env, "other-worker").await;
    let unsent = seed_incoming_follow(
        &env,
        &alice,
        &env.peer_url("/users/dave2")
            .replace("127.0.0.1", "localhost"),
        &env.peer_url("/follows/2"),
        false,
    )
    .await;

    block_domain(&env, &alice, BLOCKED_HOST).await.unwrap();

    assert_eq!(
        follow_state_by_id(&env, sending.id()).await,
        ended_blocked()
    );
    assert_eq!(follow_state_by_id(&env, unsent.id()).await, ended_blocked());
    let jobs = env.jobs_of(&alice.profile_id).await;
    let accepts = jobs_of_type(&jobs, DeliveryActivityType::Accept);
    assert!(accepts
        .iter()
        .all(|job| job.state == ProfileDeliveryJobState::Cancelled));
    let rejects = jobs_of_type(&jobs, DeliveryActivityType::Reject);
    assert_eq!(rejects.len(), 2);
    for reject in rejects {
        let delayed = reject.next_attempt_at > OffsetDateTime::now_utc() + Duration::seconds(60);
        let for_sending = body(reject)["object"]["id"] == activity;
        assert_eq!(
            delayed, for_sending,
            "only the Reject behind an in-flight Accept waits"
        );
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_peer_failure_never_undoes_the_local_block_and_detachment() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    let bob = localhost_actor(&env, "/users/bob").await;
    let outgoing = follow(&env, &alice, &alice.nanoid, &bob).await;
    deliver_all(&env).await;
    block_domain(&env, &alice, BLOCKED_HOST).await.unwrap();

    // The peer answers 500: the Undo is retried, the local state stays applied.
    env.respond_inbox(ResponseTemplate::new(500)).await;
    let report = env.cycle("w").await;
    assert_eq!((report.attempted, report.delivered), (1, 0));
    assert_eq!(follow_state(&env, &outgoing).await, ended_blocked());
    assert_eq!(
        env.count("SELECT count(*) FROM profile_domain_controls WHERE deleted_at IS NULL AND mode = 'block'")
            .await,
        1
    );
    let jobs = env.jobs_of(&alice.profile_id).await;
    let undo = jobs_of_type(&jobs, DeliveryActivityType::Undo);
    assert_eq!(undo[0].state, ProfileDeliveryJobState::Retrying);
    for request in env.inbox_requests().await {
        let value: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(
            value["type"], "Undo",
            "nothing but the clean-up is ever sent"
        );
    }

    // The peer recovers later and the Undo lands.
    env.advance_clock(Duration::hours(1));
    env.respond_inbox(ResponseTemplate::new(202)).await;
    assert_eq!(env.cycle("w").await.delivered, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_failure_while_detaching_rolls_back_the_block_and_everything_else() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    let bob = localhost_actor(&env, "/users/bob").await;
    let carol = localhost_actor(&env, "/users/carol").await;
    let a = follow(&env, &alice, &alice.nanoid, &bob).await;
    let b = follow(&env, &alice, &alice.nanoid, &carol).await;
    deliver_all(&env).await;
    let job_count = env.jobs_of(&alice.profile_id).await.len();

    // The clean-up reservation of the second relation fails: the first relation's end, the first
    // Undo and the block itself must all roll back.
    env.execute(
        "CREATE FUNCTION d8r_fail() RETURNS trigger AS $$ BEGIN RAISE EXCEPTION 'injected failure'; END $$ LANGUAGE plpgsql;
         CREATE FUNCTION d8r_fail_second() RETURNS trigger AS $$ BEGIN
           IF (SELECT count(*) FROM profile_delivery_jobs WHERE activity_type = 'Undo') >= 1 THEN
             RAISE EXCEPTION 'injected failure';
           END IF;
           RETURN NEW;
         END $$ LANGUAGE plpgsql;
         CREATE TRIGGER d8r_fail_undo BEFORE INSERT ON profile_delivery_jobs
           FOR EACH ROW WHEN (NEW.activity_type = 'Undo') EXECUTE FUNCTION d8r_fail_second();",
    )
    .await;
    assert!(block_domain(&env, &alice, BLOCKED_HOST).await.is_err());
    for nanoid in [&a, &b] {
        assert_eq!(follow_state(&env, nanoid).await.0, "pending");
    }
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), job_count);
    assert_eq!(
        env.count("SELECT count(*) FROM profile_domain_controls")
            .await,
        0,
        "the block is not stored when the detachment fails"
    );

    // A failing relation update rolls back as well (and the block is still not stored).
    env.execute("DROP TRIGGER d8r_fail_undo ON profile_delivery_jobs")
        .await;
    env.execute(
        "CREATE TRIGGER d8r_fail_follows BEFORE UPDATE ON profile_follows
           FOR EACH ROW EXECUTE FUNCTION d8r_fail()",
    )
    .await;
    assert!(block_domain(&env, &alice, BLOCKED_HOST).await.is_err());
    assert_eq!(
        env.count("SELECT count(*) FROM profile_domain_controls")
            .await,
        0
    );
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), job_count);

    // Once the fault is gone the same request applies everything at once.
    env.execute("DROP TRIGGER d8r_fail_follows ON profile_follows")
        .await;
    assert_eq!(block_domain(&env, &alice, BLOCKED_HOST).await.unwrap(), 1);
    for nanoid in [&a, &b] {
        assert_eq!(follow_state(&env, nanoid).await, ended_blocked());
    }
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), job_count + 2);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unblocking_restores_nothing_and_an_old_follow_replay_cannot_revive_a_relation() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    let bob = localhost_actor(&env, "/users/bob").await;
    let dave = format!("http://localhost:{}/users/dave", env.peer.address().port());
    let outgoing = follow(&env, &alice, &alice.nanoid, &bob).await;
    deliver_all(&env).await;
    let incoming_activity = env.peer_url("/follows/9");
    let incoming = seed_incoming_follow(&env, &alice, &dave, &incoming_activity, false).await;
    block_domain(&env, &alice, BLOCKED_HOST).await.unwrap();
    let jobs_before = env.jobs_of(&alice.profile_id).await.len();

    env.module
        .delete_domain_control(&alice.auth, None, &alice.nanoid, BLOCKED_HOST, 1)
        .await
        .unwrap();

    assert_eq!(follow_state(&env, &outgoing).await, ended_blocked());
    assert_eq!(
        follow_state_by_id(&env, incoming.id()).await,
        ended_blocked()
    );
    assert_eq!(
        env.jobs_of(&alice.profile_id).await.len(),
        jobs_before,
        "an unblock reserves no delivery and restores nothing"
    );
    assert_eq!(
        env.count("SELECT count(*) FROM profile_follows WHERE state <> 'ended'")
            .await,
        0,
        "no relation (and no restore list) exists after the unblock"
    );

    // Re-delivering the old Follow activity does not create a generation again.
    let replay = ProfileFollow::new_accepted(
        ProfileFollowId::new(kernel::generate_id()),
        ProfileGraphActor::Remote(GraphActorUrl::new(dave).unwrap()),
        ProfileGraphActor::Local(alice.profile_id.clone()),
        GraphActivityId::new(incoming_activity).unwrap(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let deps = env.module.clone();
    let outcome = env
        .module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                deps.profile_follow_repository()
                    .begin_generation(executor, &replay)
                    .await
            })
        })
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        kernel::interfaces::repository::BeginFollowOutcome::ReplayedActivity(_)
    ));
    assert_eq!(
        env.count("SELECT count(*) FROM profile_follows WHERE state <> 'ended'")
            .await,
        0
    );

    // Following again is a fresh user action and works after the unblock.
    localhost_actor(&env, "/users/bob").await;
    let again = follow(&env, &alice, &alice.nanoid, &bob).await;
    assert_eq!(follow_state(&env, &again).await.0, "pending");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn only_a_block_detaches_and_a_mute_changes_no_relation() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    let bob = localhost_actor(&env, "/users/bob").await;
    let outgoing = follow(&env, &alice, &alice.nanoid, &bob).await;
    deliver_all(&env).await;
    let jobs_before = env.jobs_of(&alice.profile_id).await.len();

    let mute = env
        .module
        .put_domain_control(
            &alice.auth,
            None,
            &alice.nanoid,
            BLOCKED_HOST,
            DomainControlMode::Mute,
            None,
        )
        .await
        .unwrap();
    assert_eq!(follow_state(&env, &outgoing).await.0, "pending");
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), jobs_before);

    // mute -> block (replace) detaches; a stale revision is a conflict that detaches nothing.
    let stale = env
        .module
        .put_domain_control(
            &alice.auth,
            None,
            &alice.nanoid,
            BLOCKED_HOST,
            DomainControlMode::Block,
            Some(mute.revision + 5),
        )
        .await;
    assert!(matches!(
        stale.unwrap_err().current_context(),
        KernelError::Concurrency
    ));
    assert_eq!(follow_state(&env, &outgoing).await.0, "pending");
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), jobs_before);
    let block = env
        .module
        .put_domain_control(
            &alice.auth,
            None,
            &alice.nanoid,
            BLOCKED_HOST,
            DomainControlMode::Block,
            Some(mute.revision),
        )
        .await
        .unwrap();
    assert_eq!(block.revision, 2);
    assert_eq!(follow_state(&env, &outgoing).await, ended_blocked());
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), jobs_before + 1);

    // block -> mute and a repeated block write do not reserve anything more.
    let back = env
        .module
        .put_domain_control(
            &alice.auth,
            None,
            &alice.nanoid,
            BLOCKED_HOST,
            DomainControlMode::Mute,
            Some(2),
        )
        .await
        .unwrap();
    let again = env
        .module
        .put_domain_control(
            &alice.auth,
            None,
            &alice.nanoid,
            BLOCKED_HOST,
            DomainControlMode::Block,
            Some(back.revision),
        )
        .await
        .unwrap();
    assert_eq!(again.revision, 4);
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), jobs_before + 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_block_applies_only_to_the_blocking_profile_and_the_exact_domain() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    // An organization Account owns two Profiles: the block of one must not reach the other.
    let org = env.seed_organization().await;
    let context = Env::org_context(&org);
    let (first_id, first_nanoid) = env.seed_profile_in(&org).await;
    let (second_id, second_nanoid) = env.seed_profile_in(&org).await;
    let bob = localhost_actor(&env, "/users/bob").await;
    let sub = format!("http://sub.localhost:{}/users/s", env.peer.address().port());
    let mine = env
        .module
        .follow_profile(&alice.auth, Some(&context), &first_nanoid, &bob)
        .await
        .unwrap()
        .nanoid;
    let sibling = env
        .module
        .follow_profile(&alice.auth, Some(&context), &second_nanoid, &bob)
        .await
        .unwrap()
        .nanoid;
    let other_account = env.seed_user().await;
    let theirs = follow(&env, &other_account, &other_account.nanoid, &bob).await;
    let sub_relation = {
        let user = User {
            auth: alice.auth.clone(),
            account: org.clone(),
            profile_id: first_id.clone(),
            nanoid: first_nanoid.clone(),
        };
        seed_bare_remote_follow(&env, &user, &sub, false).await
    };
    let put = |domain: &'static str| {
        let env = &env;
        let auth = alice.auth.clone();
        let context = context.clone();
        let nanoid = first_nanoid.clone();
        async move {
            env.module
                .put_domain_control(
                    &auth,
                    Some(&context),
                    &nanoid,
                    domain,
                    DomainControlMode::Block,
                    None,
                )
                .await
                .unwrap()
        }
    };

    // A different exact domain touches nothing here.
    put("example.test").await;
    for nanoid in [&mine, &sibling, &theirs] {
        assert_eq!(follow_state(&env, nanoid).await.0, "pending");
    }

    put(BLOCKED_HOST).await;
    assert_eq!(follow_state(&env, &mine).await, ended_blocked());
    assert_eq!(
        follow_state(&env, &sibling).await.0,
        "pending",
        "same Account, other Profile"
    );
    assert_eq!(
        follow_state(&env, &theirs).await.0,
        "pending",
        "other Account"
    );
    assert_eq!(
        follow_state_by_id(&env, sub_relation.id()).await.0,
        "accepted",
        "sub.localhost is not localhost"
    );
    for profile in [&second_id, &other_account.profile_id] {
        let jobs = env.jobs_of(profile).await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].state, ProfileDeliveryJobState::Pending);
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn blocking_this_instance_detaches_local_relations_in_both_directions_without_delivery() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let bob = env.seed_user().await;
    let carol = env.seed_user().await;
    let dave = env.seed_user().await;
    let to_bob = follow(&env, &alice, &alice.nanoid, &alice_url(&bob)).await;
    let from_carol = follow(&env, &carol, &carol.nanoid, &alice_url(&alice)).await;
    let unrelated = follow(&env, &bob, &bob.nanoid, &alice_url(&dave)).await;
    assert_eq!(follow_state(&env, &to_bob).await.0, "accepted");

    block_domain(&env, &alice, "local.example").await.unwrap();

    assert_eq!(follow_state(&env, &to_bob).await, ended_blocked());
    assert_eq!(follow_state(&env, &from_carol).await, ended_blocked());
    assert_eq!(
        follow_state(&env, &unrelated).await.0,
        "accepted",
        "a relation between two other local Profiles is not alice's"
    );
    assert_eq!(
        env.count("SELECT count(*) FROM profile_delivery_jobs")
            .await,
        0,
        "local relations need no delivery"
    );
    // And a new local Follow in either direction is refused (D8G) while the block stands.
    for (user, target) in [(&carol, &alice), (&alice, &bob)] {
        let result = env
            .module
            .follow_profile(&user.auth, None, &user.nanoid, &alice_url(target))
            .await;
        assert!(matches!(
            result.unwrap_err().current_context(),
            KernelError::Rejected
        ));
    }
}

/// A transaction that holds `profile`'s relationship lock and has stored an (uncommitted) accepted
/// relation with `follower_url`, until the returned sender fires.
fn hold_new_follower(
    env: &Arc<Env>,
    profile: kernel::prelude::entity::ProfileId,
    follower_url: String,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<ProfileFollowId>,
) {
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let module = env.module.clone();
    let holder = tokio::spawn(async move {
        let deps = module.clone();
        let follow = ProfileFollow::new_accepted(
            ProfileFollowId::new(kernel::generate_id()),
            ProfileGraphActor::Remote(GraphActorUrl::new(follower_url).unwrap()),
            ProfileGraphActor::Local(profile.clone()),
            GraphActivityId::new(format!(
                "https://localhost/follows/{}",
                kernel::generate_id()
            ))
            .unwrap(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        let id = follow.id().clone();
        module
            .transaction_manager()
            .transaction(move |executor| {
                Box::pin(async move {
                    lock_profile_for_domain_control_write(&deps, executor, &profile).await?;
                    deps.profile_follow_repository()
                        .begin_generation(executor, &follow)
                        .await?;
                    locked_tx.send(()).ok();
                    release_rx.await.ok();
                    Ok(())
                })
            })
            .await
            .unwrap();
        id
    });
    (locked_rx, release_tx, holder)
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_block_waits_for_an_in_flight_follow_and_then_detaches_the_relation_it_created() {
    let env = Arc::new(Env::new().await);
    let alice = env.seed_user().await;
    let dave = format!("http://localhost:{}/users/dave", env.peer.address().port());

    // A Follow of dave is being stored (it holds alice's lock) when the block is requested.
    let (locked, release, holder) = hold_new_follower(&env, alice.profile_id.clone(), dave);
    locked.await.unwrap();
    let mut write = tokio::spawn({
        let env = env.clone();
        let auth = alice.auth.clone();
        let nanoid = alice.nanoid.clone();
        async move {
            env.module
                .put_domain_control(
                    &auth,
                    None,
                    &nanoid,
                    BLOCKED_HOST,
                    DomainControlMode::Block,
                    None,
                )
                .await
                .map(|dto| dto.revision)
        }
    });
    assert!(
        tokio::time::timeout(StdDuration::from_millis(500), &mut write)
            .await
            .is_err(),
        "the block queues behind the Follow transaction"
    );

    release.send(()).unwrap();
    let follow_id = holder.await.unwrap();
    assert_eq!(write.await.unwrap().unwrap(), 1);
    assert_eq!(
        follow_state_by_id(&env, &follow_id).await,
        ended_blocked(),
        "block + active Follow never coexist"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_blocks_and_unfollows_leave_no_relation_and_never_deadlock() {
    let env = Arc::new(Env::new().await);
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    let mut follows = Vec::new();
    for index in 0..4 {
        let target = localhost_actor(&env, &format!("/users/u{index}")).await;
        follows.push(follow(&env, &alice, &alice.nanoid, &target).await);
    }
    deliver_all(&env).await;

    let block = tokio::spawn({
        let env = env.clone();
        let alice_auth = alice.auth.clone();
        let nanoid = alice.nanoid.clone();
        async move {
            env.module
                .put_domain_control(
                    &alice_auth,
                    None,
                    &nanoid,
                    BLOCKED_HOST,
                    DomainControlMode::Block,
                    None,
                )
                .await
                .map(|_| ())
        }
    });
    let mut unfollows = Vec::new();
    for nanoid in follows.clone() {
        let env = env.clone();
        let auth = alice.auth.clone();
        let profile = alice.nanoid.clone();
        unfollows.push(tokio::spawn(async move {
            env.module
                .unfollow_profile(&auth, None, &profile, &nanoid)
                .await
        }));
    }
    block.await.unwrap().unwrap();
    for unfollow in unfollows {
        // Either the unfollow won (it ended the relation itself) or the block did (404).
        match unfollow.await.unwrap() {
            Ok(()) => {}
            Err(report) => assert!(matches!(report.current_context(), KernelError::NotFound)),
        }
    }
    assert_eq!(
        env.count("SELECT count(*) FROM profile_follows WHERE state <> 'ended'")
            .await,
        0
    );
    // Whoever ended a relation reserved exactly one Undo for it.
    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(jobs_of_type(&jobs, DeliveryActivityType::Undo).len(), 4);
}

/// Holds the row lock of one delivery job in an open transaction (a worker that passed the pre-send
/// guard and is renewing its lease), so that a cancel of that job queues behind it.
async fn hold_job_row(env: &Env, job_id: i64) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = env.iso.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM profile_delivery_jobs WHERE id = $1 FOR UPDATE")
        .bind(job_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx
}

/// The clean-up must not become deliverable before the original activity (whose job a slow
/// operation could only cancel late) can have landed: its start is counted from after the cancel,
/// never from the time the operation began.
async fn assert_cleanup_counts_from_after_the_cancel(
    env: &Arc<Env>,
    original_job_id: i64,
    mut operation: tokio::task::JoinHandle<()>,
    cleanup: DeliveryActivityType,
    sender: &kernel::prelude::entity::ProfileId,
) {
    let tx = hold_job_row(env, original_job_id).await;
    assert!(
        tokio::time::timeout(StdDuration::from_millis(1500), &mut operation)
            .await
            .is_err(),
        "the operation is stuck on the original job's row lock"
    );
    // The original job is released only now: everything before this instant is "old".
    let released_at = OffsetDateTime::now_utc();
    tx.commit().await.unwrap();
    operation.await.unwrap();

    let jobs = env.jobs_of(sender).await;
    let cleanup_jobs = jobs_of_type(&jobs, cleanup);
    assert_eq!(cleanup_jobs.len(), 1);
    assert!(
        cleanup_jobs[0].next_attempt_at >= released_at + Duration::seconds(120),
        "the clean-up starts {:?}, earlier than 120s after the cancel could take effect ({released_at})",
        cleanup_jobs[0].next_attempt_at
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_undo_after_a_slow_block_is_counted_from_after_the_cancel_not_from_the_start() {
    let env = Arc::new(Env::new().await);
    let alice = env.seed_user().await;
    let bob = localhost_actor(&env, "/users/bob").await;
    follow(&env, &alice, &alice.nanoid, &bob).await;
    claim_one(&env, "slow-worker").await;
    let job_id = *env.jobs_of(&alice.profile_id).await[0].id.as_ref();

    let operation = tokio::spawn({
        let env = env.clone();
        let auth = alice.auth.clone();
        let nanoid = alice.nanoid.clone();
        async move {
            env.module
                .put_domain_control(
                    &auth,
                    None,
                    &nanoid,
                    BLOCKED_HOST,
                    DomainControlMode::Block,
                    None,
                )
                .await
                .unwrap();
        }
    });
    assert_cleanup_counts_from_after_the_cancel(
        &env,
        job_id,
        operation,
        DeliveryActivityType::Undo,
        &alice.profile_id,
    )
    .await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_reject_after_a_slow_block_is_counted_from_after_the_cancel_not_from_the_start() {
    let env = Arc::new(Env::new().await);
    let alice = env.seed_user().await;
    let dave = localhost_actor(&env, "/users/dave").await;
    seed_incoming_follow(&env, &alice, &dave, &env.peer_url("/follows/1"), true).await;
    claim_one(&env, "slow-worker").await;
    let job_id = *env.jobs_of(&alice.profile_id).await[0].id.as_ref();

    let operation = tokio::spawn({
        let env = env.clone();
        let auth = alice.auth.clone();
        let nanoid = alice.nanoid.clone();
        async move {
            env.module
                .put_domain_control(
                    &auth,
                    None,
                    &nanoid,
                    BLOCKED_HOST,
                    DomainControlMode::Block,
                    None,
                )
                .await
                .unwrap();
        }
    });
    assert_cleanup_counts_from_after_the_cancel(
        &env,
        job_id,
        operation,
        DeliveryActivityType::Reject,
        &alice.profile_id,
    )
    .await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_undo_after_a_slow_unfollow_is_counted_from_after_the_cancel_not_from_the_start() {
    let env = Arc::new(Env::new().await);
    let alice = env.seed_user().await;
    let bob = localhost_actor(&env, "/users/bob").await;
    let follow_nanoid = follow(&env, &alice, &alice.nanoid, &bob).await;
    claim_one(&env, "slow-worker").await;
    let job_id = *env.jobs_of(&alice.profile_id).await[0].id.as_ref();

    let operation = tokio::spawn({
        let env = env.clone();
        let auth = alice.auth.clone();
        let nanoid = alice.nanoid.clone();
        async move {
            env.module
                .unfollow_profile(&auth, None, &nanoid, &follow_nanoid)
                .await
                .unwrap();
        }
    });
    assert_cleanup_counts_from_after_the_cancel(
        &env,
        job_id,
        operation,
        DeliveryActivityType::Undo,
        &alice.profile_id,
    )
    .await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_local_relation_created_while_the_block_waits_for_the_lock_is_locked_retried_and_detached(
) {
    use super::super::domain_block::RETRIES;

    let env = Arc::new(Env::new().await);
    let alice = env.seed_user().await;
    let bob = env.seed_user().await;
    RETRIES.with(|retries| retries.set(0));

    // A Follow transaction holds alice's lock and stores bob -> alice (a local relation the block
    // could not see before taking the lock).
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let holder = tokio::spawn({
        let module = env.module.clone();
        let alice_id = alice.profile_id.clone();
        let bob_id = bob.profile_id.clone();
        async move {
            let deps = module.clone();
            let follow = ProfileFollow::new_accepted(
                ProfileFollowId::new(kernel::generate_id()),
                ProfileGraphActor::Local(bob_id),
                ProfileGraphActor::Local(alice_id.clone()),
                GraphActivityId::new(format!("{LOCAL_BASE}/follows/{}", kernel::generate_id()))
                    .unwrap(),
                OffsetDateTime::now_utc(),
            )
            .unwrap();
            let id = follow.id().clone();
            module
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        lock_profile_for_domain_control_write(&deps, executor, &alice_id).await?;
                        deps.profile_follow_repository()
                            .begin_generation(executor, &follow)
                            .await?;
                        locked_tx.send(()).ok();
                        release_rx.await.ok();
                        Ok(())
                    })
                })
                .await
                .unwrap();
            id
        }
    });
    locked_rx.await.unwrap();
    let mut write = tokio::spawn({
        let env = env.clone();
        let auth = alice.auth.clone();
        let nanoid = alice.nanoid.clone();
        async move {
            env.module
                .put_domain_control(
                    &auth,
                    None,
                    &nanoid,
                    "local.example",
                    DomainControlMode::Block,
                    None,
                )
                .await
                .map(|dto| dto.revision)
        }
    });
    assert!(
        tokio::time::timeout(StdDuration::from_millis(500), &mut write)
            .await
            .is_err(),
        "the block waits for alice's lock after reading no relation"
    );
    assert_eq!(
        env.count("SELECT count(*) FROM profile_domain_controls")
            .await,
        0
    );

    release_tx.send(()).unwrap();
    let relation = holder.await.unwrap();
    assert_eq!(
        write.await.unwrap().unwrap(),
        1,
        "the abandoned attempt wrote nothing, so the revision is not consumed"
    );
    assert_eq!(
        RETRIES.with(|retries| retries.get()),
        1,
        "the first attempt noticed the unlocked local counterpart and started over"
    );
    assert_eq!(
        follow_state_by_id(&env, &relation).await,
        ended_blocked(),
        "the retry locked bob too and detached the relation"
    );
    assert_eq!(
        env.count("SELECT count(*) FROM profile_domain_controls WHERE revision = 1")
            .await,
        1
    );

    // The final unblock restores nothing.
    env.module
        .delete_domain_control(&alice.auth, None, &alice.nanoid, "local.example", 1)
        .await
        .unwrap();
    assert_eq!(follow_state_by_id(&env, &relation).await, ended_blocked());
}
