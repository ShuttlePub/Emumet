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
    let mut executor = env.module.database_connection().connection().await.unwrap();
    let claimed = env
        .module
        .profile_delivery_job_repository()
        .claim_due(
            &mut executor,
            "other-worker",
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
