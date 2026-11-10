//! D8G: domain `block` applied to Follow on the sending side (`follow_profile`), before delivery
//! (`cancel_if_recipient_domain_blocked`) and in domain-control writes (lock convention).
//!
//! Remote peers are the wiremock server reached through the name `localhost` (the server listens
//! on 127.0.0.1): a domain control cannot be stored for an IP literal, and `localhost` is an exact
//! host the D8P matcher understands. The peer's inbox deliberately stays `http://127.0.0.1:…` so
//! the tests also show that the inbox host is never consulted.

use super::support::{Env, User, LOCAL_BASE};
use crate::service::domain_control::DomainControlUseCase;
use crate::service::domain_follow_guard::lock_profile_for_domain_control_write;
use crate::service::profile_relationship::{
    DeliverProfileActivitiesUseCase, ProfileRelationshipUseCase,
};
use kernel::interfaces::database::{DependOnTransactionManager, TransactionManager};
use kernel::interfaces::repository::{
    DependOnDomainControlRepository, DependOnProfileDeliveryEnqueuer,
    DependOnProfileDeliveryJobRepository, DependOnProfileFollowRepository, DomainControlRepository,
    ProfileDeliveryEnqueuer, ProfileDeliveryJobRepository, ProfileFollowRepository,
};
use kernel::prelude::entity::{
    ControlDomain, DeliveryActivityType, DeliveryClaimPolicy, DomainControlMode, GraphActivityId,
    GraphActorUrl, GraphInboxUrl, NewProfileDeliveryJob, ProfileDeliveryJobState, ProfileFollow,
    ProfileFollowId, ProfileGraphActor, ProfileId,
};
use kernel::KernelError;
use std::time::Duration as StdDuration;
use time::{Duration, OffsetDateTime};
use wiremock::ResponseTemplate;

fn actor_url(user: &User) -> String {
    format!("{LOCAL_BASE}/ap/profiles/{}", user.nanoid)
}

fn rejected<T: std::fmt::Debug>(result: error_stack::Result<T, KernelError>) -> bool {
    match result {
        Ok(value) => panic!("expected a refusal, got {value:?}"),
        Err(report) => matches!(report.current_context(), KernelError::Rejected),
    }
}

/// An actor of the peer reached through the host name `localhost`.
async fn localhost_actor(env: &Env, path: &str) -> String {
    let url = format!("http://localhost:{}{path}", env.peer.address().port());
    env.mount_actor_as(path, &url).await;
    url
}

async fn set_control(env: &Env, profile: &ProfileId, domain: &str, mode: DomainControlMode) {
    let deps = env.module.clone();
    let profile = profile.clone();
    let domain = ControlDomain::parse(domain).unwrap();
    env.module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                deps.domain_control_repository()
                    .create(executor, &profile, &domain, mode)
                    .await
                    .map(|_| ())
            })
        })
        .await
        .unwrap();
}

async fn clear_controls(env: &Env) {
    env.execute("DELETE FROM profile_domain_controls").await;
}

async fn follow_state(env: &Env, activity_like: &str) -> Vec<(String, Option<String>)> {
    sqlx::query_as(
        "SELECT state, end_reason FROM profile_follows WHERE activity_id LIKE $1 ORDER BY id",
    )
    .bind(format!("%{activity_like}%"))
    .fetch_all(&env.iso.pool)
    .await
    .unwrap()
}

/// Holds the relationship lock of `profile` in an open transaction (a Follow or block write in
/// flight) until the returned sender fires.
fn hold_lock(
    env: &Env,
    profile: ProfileId,
    block_domain: Option<&'static str>,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let module = env.module.clone();
    let holder = tokio::spawn(async move {
        let deps = module.clone();
        module
            .transaction_manager()
            .transaction(move |executor| {
                Box::pin(async move {
                    lock_profile_for_domain_control_write(&deps, executor, &profile).await?;
                    if let Some(domain) = block_domain {
                        deps.domain_control_repository()
                            .create(
                                executor,
                                &profile,
                                &ControlDomain::parse(domain).unwrap(),
                                DomainControlMode::Block,
                            )
                            .await?;
                    }
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
    tokio::time::timeout(StdDuration::from_millis(500), handle)
        .await
        .is_err()
}

/// A peer (`localhost`) Follow of `alice` with a pending Follow job and a signing key.
async fn pending_follow(env: &Env) -> (User, String, String) {
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    let target = localhost_actor(env, "/users/bob").await;
    let dto = env
        .module
        .follow_profile(&alice.auth, None, &alice.nanoid, &target)
        .await
        .unwrap();
    (alice, target, dto.nanoid)
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn outbound_follow_to_a_blocked_domain_is_refused_and_creates_nothing() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let target = localhost_actor(&env, "/users/bob").await;
    set_control(
        &env,
        &alice.profile_id,
        "localhost",
        DomainControlMode::Block,
    )
    .await;

    let result = env
        .module
        .follow_profile(&alice.auth, None, &alice.nanoid, &target)
        .await;

    assert!(rejected(result), "refused as a rejected (422) request");
    assert_eq!(env.count("SELECT count(*) FROM profile_follows").await, 0);
    assert_eq!(
        env.count("SELECT count(*) FROM profile_delivery_jobs")
            .await,
        0
    );

    // Removing the block does not conjure a relation; a new Follow is a user action.
    clear_controls(&env).await;
    assert_eq!(env.count("SELECT count(*) FROM profile_follows").await, 0);
    env.module
        .follow_profile(&alice.auth, None, &alice.nanoid, &target)
        .await
        .expect("after the unblock a new Follow is accepted");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn mute_and_other_domains_do_not_refuse_an_outbound_follow() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let target = localhost_actor(&env, "/users/bob").await;
    set_control(
        &env,
        &alice.profile_id,
        "localhost",
        DomainControlMode::Mute,
    )
    .await;
    set_control(
        &env,
        &alice.profile_id,
        "elsewhere.example",
        DomainControlMode::Block,
    )
    .await;

    env.module
        .follow_profile(&alice.auth, None, &alice.nanoid, &target)
        .await
        .expect("a mute and a block of another domain never refuse");
    assert_eq!(env.count("SELECT count(*) FROM profile_follows").await, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_block_of_one_profile_does_not_affect_other_profiles() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let org = env.seed_organization().await;
    let context = Env::org_context(&org);
    let (org_profile_a, org_nanoid_a) = env.seed_profile_in(&org).await;
    let (_org_profile_b, org_nanoid_b) = env.seed_profile_in(&org).await;
    let carol = env.seed_user().await;
    let target = localhost_actor(&env, "/users/bob").await;
    set_control(&env, &org_profile_a, "localhost", DomainControlMode::Block).await;

    // Same (organization) Account, other Profile: unaffected.
    env.module
        .follow_profile(&alice.auth, Some(&context), &org_nanoid_b, &target)
        .await
        .expect("another Profile of the same Account is not blocked");
    env.module
        .follow_profile(&carol.auth, None, &carol.nanoid, &target)
        .await
        .expect("a Profile of another Account is not blocked");
    assert!(rejected(
        env.module
            .follow_profile(&alice.auth, Some(&context), &org_nanoid_a, &target)
            .await
    ));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn local_follows_check_both_profiles_policies() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let bob = env.seed_user().await;
    let local_host = "local.example";

    // The followee's block of this instance's host refuses the Follow of a local follower.
    set_control(&env, &bob.profile_id, local_host, DomainControlMode::Block).await;
    assert!(rejected(
        env.module
            .follow_profile(&alice.auth, None, &alice.nanoid, &actor_url(&bob))
            .await
    ));
    // ... and the blocking Profile's own outgoing Follow too (its block applies to the host).
    assert!(rejected(
        env.module
            .follow_profile(&bob.auth, None, &bob.nanoid, &actor_url(&alice))
            .await
    ));
    assert_eq!(env.count("SELECT count(*) FROM profile_follows").await, 0);

    // A mute does not refuse.
    clear_controls(&env).await;
    set_control(&env, &bob.profile_id, local_host, DomainControlMode::Mute).await;
    env.module
        .follow_profile(&alice.auth, None, &alice.nanoid, &actor_url(&bob))
        .await
        .expect("mute never refuses a local Follow");

    // The follower's own block refuses as well.
    clear_controls(&env).await;
    let carol = env.seed_user().await;
    set_control(
        &env,
        &carol.profile_id,
        local_host,
        DomainControlMode::Block,
    )
    .await;
    assert!(rejected(
        env.module
            .follow_profile(&carol.auth, None, &carol.nanoid, &actor_url(&bob))
            .await
    ));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn opposite_local_follows_at_the_same_time_do_not_deadlock() {
    let env = Env::new().await;
    for round in 0..12 {
        let alice = env.seed_user().await;
        let bob = env.seed_user().await;
        let alice_url = actor_url(&alice);
        let bob_url = actor_url(&bob);
        let (first, second) = tokio::join!(
            tokio::time::timeout(
                StdDuration::from_secs(30),
                env.module
                    .follow_profile(&alice.auth, None, &alice.nanoid, &bob_url),
            ),
            tokio::time::timeout(
                StdDuration::from_secs(30),
                env.module
                    .follow_profile(&bob.auth, None, &bob.nanoid, &alice_url),
            ),
        );
        first
            .unwrap_or_else(|_| panic!("round {round}: A -> B timed out (deadlock?)"))
            .expect("A -> B");
        second
            .unwrap_or_else(|_| panic!("round {round}: B -> A timed out (deadlock?)"))
            .expect("B -> A");
    }
    assert_eq!(
        env.count("SELECT count(*) FROM profile_follows WHERE state = 'accepted'")
            .await,
        24
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_follow_waits_for_an_in_flight_domain_block_write_and_is_then_refused() {
    let env = std::sync::Arc::new(Env::new().await);
    let alice = env.seed_user().await;
    let target = localhost_actor(&env, "/users/bob").await;

    // The block write took the lock and stored the control, but has not committed.
    let (locked, release, holder) = hold_lock(&env, alice.profile_id.clone(), Some("localhost"));
    locked.await.unwrap();

    let mut follow = tokio::spawn({
        let env = env.clone();
        let auth = alice.auth.clone();
        let nanoid = alice.nanoid.clone();
        async move {
            env.module
                .follow_profile(&auth, None, &nanoid, &target)
                .await
                .map(|_| ())
        }
    });
    assert!(
        still_waiting(&mut follow).await,
        "the Follow must wait for the block write instead of racing it"
    );
    assert_eq!(env.count("SELECT count(*) FROM profile_follows").await, 0);

    release.send(()).unwrap();
    holder.await.unwrap();
    assert!(rejected(follow.await.unwrap().map(|_| ())));
    assert_eq!(env.count("SELECT count(*) FROM profile_follows").await, 0);
    assert_eq!(
        env.count("SELECT count(*) FROM profile_delivery_jobs")
            .await,
        0
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_domain_control_write_waits_for_a_follow_transaction_holding_the_profile_lock() {
    let env = std::sync::Arc::new(Env::new().await);
    let alice = env.seed_user().await;

    // A Follow of alice is in flight (it holds her relationship lock).
    let (locked, release, holder) = hold_lock(&env, alice.profile_id.clone(), None);
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
                    "localhost",
                    DomainControlMode::Block,
                    None,
                )
                .await
                .map(|dto| dto.revision)
        }
    });
    assert!(
        still_waiting(&mut write).await,
        "a domain block write must queue behind the Follow transaction"
    );
    assert_eq!(
        env.count("SELECT count(*) FROM profile_domain_controls")
            .await,
        0
    );

    release.send(()).unwrap();
    holder.await.unwrap();
    assert_eq!(write.await.unwrap().unwrap(), 1);

    // The delete path takes the same lock.
    let (locked, release, holder) = hold_lock(&env, alice.profile_id.clone(), None);
    locked.await.unwrap();
    let mut delete = tokio::spawn({
        let env = env.clone();
        let auth = alice.auth.clone();
        let nanoid = alice.nanoid.clone();
        async move {
            env.module
                .delete_domain_control(&auth, None, &nanoid, "localhost", 1)
                .await
        }
    });
    assert!(still_waiting(&mut delete).await);
    release.send(()).unwrap();
    holder.await.unwrap();
    delete.await.unwrap().unwrap();
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_unsent_follow_is_cancelled_before_delivery_while_the_domain_is_blocked() {
    let env = Env::new().await;
    let (alice, _target, follow_nanoid) = pending_follow(&env).await;
    env.respond_inbox(ResponseTemplate::new(202)).await;
    set_control(
        &env,
        &alice.profile_id,
        "localhost",
        DomainControlMode::Block,
    )
    .await;

    let report = env.cycle("worker-1").await;

    assert_eq!(
        (report.attempted, report.delivered, report.domain_blocked),
        (1, 0, 1)
    );
    assert!(
        env.inbox_requests().await.is_empty(),
        "nothing reached the peer"
    );
    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].state, ProfileDeliveryJobState::Cancelled);
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT state, end_reason FROM profile_follows WHERE nanoid = $1")
            .bind(&follow_nanoid)
            .fetch_one(&env.iso.pool)
            .await
            .unwrap();
    assert_eq!(row, ("ended".to_string(), Some("blocked".to_string())));

    // Unblocking does not revive the Follow or the job.
    clear_controls(&env).await;
    assert_eq!(env.cycle("worker-1").await.attempted, 0);
    assert!(env.inbox_requests().await.is_empty());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_retrying_follow_is_cancelled_when_a_block_arrives_between_attempts() {
    let env = Env::new().await;
    let (alice, _target, _follow) = pending_follow(&env).await;
    env.respond_inbox(ResponseTemplate::new(503)).await;
    let first = env.cycle("worker-1").await;
    assert_eq!((first.attempted, first.retrying), (1, 1));
    assert_eq!(env.inbox_requests().await.len(), 1);

    set_control(
        &env,
        &alice.profile_id,
        "localhost",
        DomainControlMode::Block,
    )
    .await;
    env.advance_clock(Duration::hours(2));
    let second = env.cycle("worker-1").await;

    assert_eq!(second.domain_blocked, 1);
    assert_eq!(env.inbox_requests().await.len(), 1, "no second attempt");
    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(jobs[0].state, ProfileDeliveryJobState::Cancelled);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn mute_and_other_profiles_blocks_do_not_stop_delivery() {
    let env = Env::new().await;
    let (alice, _target, _follow) = pending_follow(&env).await;
    let bob = env.seed_user().await;
    env.respond_inbox(ResponseTemplate::new(202)).await;
    set_control(
        &env,
        &alice.profile_id,
        "localhost",
        DomainControlMode::Mute,
    )
    .await;
    set_control(&env, &bob.profile_id, "localhost", DomainControlMode::Block).await;

    let report = env.cycle("worker-1").await;

    assert_eq!((report.delivered, report.domain_blocked), (1, 0));
    assert_eq!(env.inbox_requests().await.len(), 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_unsent_accept_is_cancelled_and_its_generation_ends() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    let bob_url = localhost_actor(&env, "/users/bob").await;
    let incoming = ProfileFollow::new_accepted(
        ProfileFollowId::new(kernel::generate_id()),
        ProfileGraphActor::Remote(GraphActorUrl::new(bob_url.as_str()).unwrap()),
        ProfileGraphActor::Local(alice.profile_id.clone()),
        GraphActivityId::new(format!("{bob_url}#follows/9")).unwrap(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let accept_job = NewProfileDeliveryJob::new(
        alice.profile_id.clone(),
        DeliveryActivityType::Accept,
        GraphActivityId::new(format!("{}/activities/accept-1", actor_url(&alice))).unwrap(),
        GraphInboxUrl::new(env.peer_url("/inbox")).unwrap(),
        "{\"type\":\"Accept\"}",
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .with_follow(incoming.id().clone());
    {
        let deps = env.module.clone();
        let incoming = incoming.clone();
        env.module
            .transaction_manager()
            .transaction(move |executor| {
                Box::pin(async move {
                    deps.profile_follow_repository()
                        .begin_generation(executor, &incoming)
                        .await?;
                    deps.profile_delivery_enqueuer()
                        .enqueue(executor, &accept_job)
                        .await
                        .map(|_| ())
                })
            })
            .await
            .unwrap();
    }
    env.respond_inbox(ResponseTemplate::new(202)).await;
    set_control(
        &env,
        &alice.profile_id,
        "localhost",
        DomainControlMode::Block,
    )
    .await;

    let report = env.cycle("worker-1").await;

    assert_eq!(report.domain_blocked, 1);
    assert!(env.inbox_requests().await.is_empty());
    assert_eq!(
        env.jobs_of(&alice.profile_id).await[0].state,
        ProfileDeliveryJobState::Cancelled
    );
    assert_eq!(
        follow_state(&env, "#follows/9").await,
        vec![("ended".to_string(), Some("blocked".to_string()))]
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn undo_and_reject_clean_up_is_still_delivered_while_the_domain_is_blocked() {
    let env = Env::new().await;
    let (alice, _target, follow_nanoid) = pending_follow(&env).await;
    env.respond_inbox(ResponseTemplate::new(202)).await;
    assert_eq!(env.cycle("worker-1").await.delivered, 1);

    set_control(
        &env,
        &alice.profile_id,
        "localhost",
        DomainControlMode::Block,
    )
    .await;
    env.module
        .unfollow_profile(&alice.auth, None, &alice.nanoid, &follow_nanoid)
        .await
        .unwrap();
    let report = env.cycle("worker-1").await;

    assert_eq!((report.delivered, report.domain_blocked), (1, 0));
    let requests = env.inbox_requests().await;
    assert_eq!(requests.len(), 2);
    let body: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(body["type"], "Undo");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn the_pre_send_decision_waits_for_the_sender_lock() {
    let env = std::sync::Arc::new(Env::new().await);
    let (alice, _target, _follow) = pending_follow(&env).await;
    let claim = {
        let mut executor = {
            use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
            env.module.database_connection().connection().await.unwrap()
        };
        env.module
            .profile_delivery_job_repository()
            .claim_due(
                &mut executor,
                "worker-1",
                env.now(),
                &DeliveryClaimPolicy {
                    lease_duration: Duration::seconds(120),
                    limit: 1,
                    max_attempts: 12,
                },
            )
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("the Follow job is claimable")
    };

    // A block write for alice is in flight (uncommitted); the decision must queue behind it and
    // then see the committed block.
    let (locked, release, holder) = hold_lock(&env, alice.profile_id.clone(), Some("localhost"));
    locked.await.unwrap();
    let mut decision = tokio::spawn({
        let env = env.clone();
        let claim = claim.clone();
        async move {
            env.module
                .cancel_if_recipient_domain_blocked(&claim, OffsetDateTime::now_utc())
                .await
                .unwrap()
        }
    });
    assert!(still_waiting(&mut decision).await);
    release.send(()).unwrap();
    holder.await.unwrap();
    assert!(
        decision.await.unwrap(),
        "cancelled after the block committed"
    );
}
