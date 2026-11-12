use super::support::{Env, User, LOCAL_BASE};
use crate::dto::profile_inbound::ProfileFollowDirection;
use crate::dto::profile_relationship::ProfileFollowDto;
use crate::service::profile_follow_list::ProfileFollowListUseCase;
use crate::service::profile_relationship::ProfileRelationshipUseCase;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnDatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::repository::DependOnProfileDeliveryEnqueuer;
use kernel::interfaces::repository::{
    DependOnDomainControlRepository, DependOnProfileDeliveryJobRepository,
    DependOnProfileFollowRepository, DomainControlRepository, ProfileDeliveryEnqueuer,
    ProfileDeliveryJobRepository, ProfileFollowRepository,
};
use kernel::prelude::entity::{
    ControlDomain, DeliveryActivityType, DomainControlMode, GraphActivityId, GraphActorUrl,
    GraphInboxUrl, NewProfileDeliveryJob, ProfileDeliveryJob, ProfileDeliveryJobState,
    ProfileFollow, ProfileFollowId, ProfileFollowState, ProfileGraphActor,
};
use kernel::KernelError;
use serde_json::Value;
use time::{Duration, OffsetDateTime};
use wiremock::ResponseTemplate;

type TestResult<T> = error_stack::Result<T, KernelError>;

fn context<T: std::fmt::Debug>(result: TestResult<T>) -> KernelError {
    match result {
        Ok(value) => panic!("expected an error, got {value:?}"),
        Err(report) => match report.current_context() {
            KernelError::Concurrency => KernelError::Concurrency,
            KernelError::Timeout => KernelError::Timeout,
            KernelError::Internal => KernelError::Internal,
            KernelError::PermissionDenied => KernelError::PermissionDenied,
            KernelError::NotFound => KernelError::NotFound,
            KernelError::Rejected => KernelError::Rejected,
            KernelError::Validation => KernelError::Validation,
        },
    }
}

fn actor_url(user: &User) -> String {
    format!("{LOCAL_BASE}/ap/profiles/{}", user.nanoid)
}

async fn follow(env: &Env, user: &User, target: &str) -> TestResult<ProfileFollowDto> {
    env.module
        .follow_profile(&user.auth, None, &user.nanoid, target)
        .await
}

async fn unfollow(env: &Env, user: &User, follow_nanoid: &str) -> TestResult<()> {
    env.module
        .unfollow_profile(&user.auth, None, &user.nanoid, follow_nanoid)
        .await
}

fn body(job: &ProfileDeliveryJob) -> Value {
    serde_json::from_str(&job.payload).unwrap()
}

async fn follow_row(env: &Env, nanoid: &str) -> (String, Option<String>) {
    sqlx::query_as("SELECT state, end_reason FROM profile_follows WHERE nanoid = $1")
        .bind(nanoid)
        .fetch_one(&env.iso.pool)
        .await
        .unwrap()
}

/// bob → alice の受理済み Follow を、P3i が保存し Accept を予約した状態として再現する。
async fn seed_incoming_follow(
    env: &Env,
    alice: &User,
    bob_url: &str,
) -> (ProfileFollow, GraphActivityId) {
    let incoming_activity = GraphActivityId::new(env.peer_url("/follows/9")).unwrap();
    let incoming = ProfileFollow::new_accepted(
        ProfileFollowId::new(kernel::generate_id()),
        ProfileGraphActor::Remote(GraphActorUrl::new(bob_url).unwrap()),
        ProfileGraphActor::Local(alice.profile_id.clone()),
        incoming_activity.clone(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let accept_job = NewProfileDeliveryJob::new(
        alice.profile_id.clone(),
        DeliveryActivityType::Accept,
        GraphActivityId::new(format!("{}/activities/accept-1", actor_url(alice))).unwrap(),
        GraphInboxUrl::new(env.peer_url("/inbox")).unwrap(),
        "{\"type\":\"Accept\"}",
        OffsetDateTime::now_utc() + Duration::hours(1),
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
    (incoming, incoming_activity)
}

/// 配送 job を全部 `Delivered` にする (peer が 202 を返す前提で worker を回す)。
async fn deliver_all(env: &Env) {
    env.respond_inbox(ResponseTemplate::new(202)).await;
    for _ in 0..5 {
        if env.cycle("w").await.attempted == 0 {
            break;
        }
    }
}

async fn accept(env: &Env, follow_id: &ProfileFollowId) {
    let deps = env.module.clone();
    let id = follow_id.clone();
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

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn follow_stores_a_pending_generation_and_reserves_the_follow_job_together() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.mount_actor("/users/bob").await;
    let target = env.peer_url("/users/bob");

    let dto = follow(&env, &alice, &target).await.unwrap();

    assert_eq!(dto.state, ProfileFollowState::Pending);
    assert_eq!(dto.target, target);
    assert_eq!(dto.profile_nanoid, alice.nanoid);
    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.activity_type, DeliveryActivityType::Follow);
    assert_eq!(job.state, ProfileDeliveryJobState::Pending);
    assert_eq!(job.target_inbox.as_str(), env.peer_url("/inbox"));
    assert!(job.follow_id.is_some());
    let payload = body(job);
    assert_eq!(payload["type"], "Follow");
    assert_eq!(payload["actor"], actor_url(&alice));
    assert_eq!(payload["object"], target);
    assert_eq!(payload["id"], job.activity_id.as_str());
    assert_eq!(follow_row(&env, &dto.nanoid).await.0, "pending");
    // 二重に Follow しても有効な世代は 1 つで、job も増えない。
    assert_eq!(
        context(follow(&env, &alice, &target).await),
        KernelError::Concurrency
    );
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_failed_job_reservation_leaves_no_follow_and_a_failed_follow_change_leaves_no_job() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.mount_actor("/users/bob").await;
    let target = env.peer_url("/users/bob");
    env.execute(
        "CREATE FUNCTION p3o_fail() RETURNS trigger AS $$ BEGIN RAISE EXCEPTION 'injected failure'; END $$ LANGUAGE plpgsql;
         CREATE TRIGGER p3o_fail_jobs BEFORE INSERT ON profile_delivery_jobs FOR EACH ROW EXECUTE FUNCTION p3o_fail();",
    )
    .await;

    assert!(follow(&env, &alice, &target).await.is_err());
    assert_eq!(env.count("SELECT count(*) FROM profile_follows").await, 0);
    assert_eq!(
        env.count("SELECT count(*) FROM profile_delivery_jobs")
            .await,
        0
    );

    env.execute("DROP TRIGGER p3o_fail_jobs ON profile_delivery_jobs")
        .await;
    let dto = follow(&env, &alice, &target).await.unwrap();
    deliver_all(&env).await;
    env.execute(
        "CREATE TRIGGER p3o_fail_follows BEFORE UPDATE ON profile_follows FOR EACH ROW EXECUTE FUNCTION p3o_fail()",
    )
    .await;

    assert!(unfollow(&env, &alice, &dto.nanoid).await.is_err());
    assert!(env
        .module
        .block_actor(&alice.auth, None, &alice.nanoid, &target)
        .await
        .is_err());
    assert_eq!(follow_row(&env, &dto.nanoid).await.0, "pending");
    assert_eq!(
        env.jobs_of(&alice.profile_id).await.len(),
        1,
        "no Undo job remains"
    );
    assert_eq!(env.count("SELECT count(*) FROM profile_blocks").await, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unfollow_before_any_delivery_cancels_the_follow_and_sends_no_undo() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.mount_actor("/users/bob").await;
    let dto = follow(&env, &alice, &env.peer_url("/users/bob"))
        .await
        .unwrap();

    unfollow(&env, &alice, &dto.nanoid).await.unwrap();

    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].state, ProfileDeliveryJobState::Cancelled);
    assert_eq!(jobs[0].attempt_count, 0);
    assert_eq!(
        follow_row(&env, &dto.nanoid).await,
        ("ended".to_string(), Some("undone".to_string()))
    );
    env.respond_inbox(ResponseTemplate::new(202)).await;
    assert_eq!(env.cycle("w").await.attempted, 0);
    assert!(env.inbox_requests().await.is_empty());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unfollow_after_delivery_sends_an_undo_that_references_the_original_follow() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let dto = follow(&env, &alice, &env.peer_url("/users/bob"))
        .await
        .unwrap();
    deliver_all(&env).await;

    unfollow(&env, &alice, &dto.nanoid).await.unwrap();

    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(jobs.len(), 2);
    let (follow_job, undo_job) = (&jobs[0], &jobs[1]);
    assert_eq!(follow_job.state, ProfileDeliveryJobState::Delivered);
    assert_eq!(undo_job.activity_type, DeliveryActivityType::Undo);
    assert_eq!(undo_job.target_inbox, follow_job.target_inbox);
    assert_eq!(
        undo_job.activity_id.as_str(),
        format!("{}/undo", follow_job.activity_id.as_str())
    );
    let undo = body(undo_job);
    assert_eq!(undo["type"], "Undo");
    assert_eq!(undo["actor"], actor_url(&alice));
    assert_eq!(undo["object"]["type"], "Follow");
    assert_eq!(undo["object"]["id"], follow_job.activity_id.as_str());
    assert_eq!(undo["object"]["object"], env.peer_url("/users/bob"));
    assert_eq!(
        context(unfollow(&env, &alice, &dto.nanoid).await),
        KernelError::NotFound,
        "unfollowing twice does not create a second Undo"
    );
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), 2);

    deliver_all(&env).await;
    let requests = env.inbox_requests().await;
    assert_eq!(
        requests.len(),
        1,
        "deliver_all reset the peer; only the Undo is recorded"
    );
    let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(sent["type"], "Undo");
    assert_eq!(sent["object"]["id"], follow_job.activity_id.as_str());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_unfollow_stays_applied_when_the_remote_delivery_fails() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let dto = follow(&env, &alice, &env.peer_url("/users/bob"))
        .await
        .unwrap();
    deliver_all(&env).await;
    unfollow(&env, &alice, &dto.nanoid).await.unwrap();

    env.respond_inbox(ResponseTemplate::new(500)).await;
    let report = env.cycle("w").await;

    assert_eq!(report.retrying, 1);
    assert_eq!(
        follow_row(&env, &dto.nanoid).await,
        ("ended".to_string(), Some("undone".to_string())),
        "a delivery failure never restores the relationship"
    );
    let undo = &env.jobs_of(&alice.profile_id).await[1];
    assert_eq!(undo.state, ProfileDeliveryJobState::Retrying);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_undo_waits_for_a_follow_that_may_still_be_in_flight() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.mount_actor("/users/bob").await;
    let dto = follow(&env, &alice, &env.peer_url("/users/bob"))
        .await
        .unwrap();
    // 別 worker が Follow を送信中 (lease 保持) の状態を作る。
    let claim_policy = kernel::prelude::entity::DeliveryClaimPolicy {
        lease_duration: Duration::seconds(60),
        limit: 1,
        max_attempts: 12,
    };
    {
        let mut executor = env.module.database_connection().connection().await.unwrap();
        let claimed = env
            .module
            .profile_delivery_job_repository()
            .claim_due(
                &mut executor,
                "crashed-worker",
                OffsetDateTime::now_utc(),
                &claim_policy,
            )
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
    }

    unfollow(&env, &alice, &dto.nanoid).await.unwrap();

    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(jobs[0].state, ProfileDeliveryJobState::Cancelled);
    assert_eq!(jobs[1].activity_type, DeliveryActivityType::Undo);
    assert!(
        jobs[1].next_attempt_at > OffsetDateTime::now_utc() + Duration::seconds(60),
        "the Undo is sent only after the in-flight Follow could have landed"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn only_the_current_owner_can_act_and_foreign_follows_are_not_found() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let mallory = env.seed_user().await;
    env.mount_actor("/users/bob").await;
    let target = env.peer_url("/users/bob");

    assert_eq!(
        context(
            env.module
                .follow_profile(&mallory.auth, None, &alice.nanoid, &target)
                .await
        ),
        KernelError::PermissionDenied
    );
    assert_eq!(
        context(
            env.module
                .follow_profile(&alice.auth, None, "no-such-profile", &target)
                .await
        ),
        KernelError::NotFound
    );
    assert_eq!(env.count("SELECT count(*) FROM profile_follows").await, 0);

    let dto = follow(&env, &alice, &target).await.unwrap();
    assert_eq!(
        context(
            env.module
                .unfollow_profile(&mallory.auth, None, &alice.nanoid, &dto.nanoid)
                .await
        ),
        KernelError::PermissionDenied
    );
    assert_eq!(
        context(unfollow(&env, &mallory, &dto.nanoid).await),
        KernelError::NotFound,
        "mallory's own Profile does not own alice's follow"
    );
    assert_eq!(follow_row(&env, &dto.nanoid).await.0, "pending");

    // 組織の Profile は組織 context でのみ操作できる。
    let org = env.seed_organization().await;
    let (org_profile, org_nanoid) = env.seed_profile_in(&org).await;
    let context_org = Env::org_context(&org);
    assert_eq!(
        context(
            env.module
                .follow_profile(&alice.auth, None, &org_nanoid, &target)
                .await
        ),
        KernelError::PermissionDenied
    );
    env.module
        .follow_profile(&alice.auth, Some(&context_org), &org_nanoid, &target)
        .await
        .unwrap();
    assert_eq!(env.jobs_of(&org_profile).await.len(), 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn profiles_of_one_account_do_not_share_follows_blocks_or_mutes() {
    let env = Env::new().await;
    let caller = env.seed_user().await;
    let org = env.seed_organization().await;
    let (first, first_nanoid) = env.seed_profile_in(&org).await;
    let (second, second_nanoid) = env.seed_profile_in(&org).await;
    let org_context = Env::org_context(&org);
    env.mount_actor("/users/bob").await;
    let target = env.peer_url("/users/bob");

    env.module
        .block_actor(&caller.auth, Some(&org_context), &first_nanoid, &target)
        .await
        .unwrap();
    env.module
        .mute_actor(&caller.auth, Some(&org_context), &first_nanoid, &target)
        .await
        .unwrap();

    assert_eq!(
        context(
            env.module
                .follow_profile(&caller.auth, Some(&org_context), &first_nanoid, &target)
                .await
        ),
        KernelError::Rejected
    );
    env.module
        .follow_profile(&caller.auth, Some(&org_context), &second_nanoid, &target)
        .await
        .expect("the second Profile of the same Account is not affected by the first one's block");
    // The first Profile's block of the remote actor is announced with an AP Block; nothing else.
    let first_jobs = env.jobs_of(&first).await;
    assert_eq!(first_jobs.len(), 1);
    assert_eq!(first_jobs[0].activity_type, DeliveryActivityType::Block);
    assert_eq!(env.jobs_of(&second).await.len(), 1);
    let list = env
        .module
        .list_profile_follows(
            &caller.auth,
            Some(&org_context),
            &first_nanoid,
            ProfileFollowDirection::Following,
            None,
            Some(10),
        )
        .await
        .unwrap();
    assert!(list.items.is_empty());
    let list = env
        .module
        .list_profile_follows(
            &caller.auth,
            Some(&org_context),
            &second_nanoid,
            ProfileFollowDirection::Following,
            None,
            Some(10),
        )
        .await
        .unwrap();
    assert_eq!(list.items.len(), 1);
    // 自分の Profile 側にも波及しない。
    assert_eq!(env.count("SELECT count(*) FROM profile_blocks").await, 1);
    assert!(env.jobs_of(&caller.profile_id).await.is_empty());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_block_refuses_follows_in_both_directions_between_local_profiles() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let bob = env.seed_user().await;

    let block = env
        .module
        .block_actor(&alice.auth, None, &alice.nanoid, &actor_url(&bob))
        .await
        .unwrap();
    assert_eq!(block.target, actor_url(&bob));

    assert_eq!(
        context(follow(&env, &alice, &actor_url(&bob)).await),
        KernelError::Rejected
    );
    assert_eq!(
        context(follow(&env, &bob, &actor_url(&alice)).await),
        KernelError::Rejected,
        "the blocked side cannot follow the blocker either"
    );
    assert_eq!(env.count("SELECT count(*) FROM profile_follows").await, 0);

    env.module
        .unblock_actor(&alice.auth, None, &alice.nanoid, &block.nanoid)
        .await
        .unwrap();
    let dto = follow(&env, &bob, &actor_url(&alice)).await.unwrap();
    assert_eq!(dto.target, actor_url(&alice));
}

async fn local_follow_row(env: &Env, nanoid: &str) -> (String, bool, bool, Option<String>) {
    sqlx::query_as(
        "SELECT state, (follower_profile_id IS NOT NULL AND followee_profile_id IS NOT NULL), (follower_actor_url IS NULL AND followee_actor_url IS NULL), end_reason FROM profile_follows WHERE nanoid = $1",
    )
    .bind(nanoid)
    .fetch_one(&env.iso.pool)
    .await
    .unwrap()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn following_a_local_profile_completes_in_one_transaction_without_http() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let bob = env.seed_user().await;
    let carol = env.seed_user().await;

    let dto = follow(&env, &bob, &actor_url(&alice)).await.unwrap();

    assert_eq!(
        dto.state,
        ProfileFollowState::Accepted,
        "no Accept round trip is needed"
    );
    assert_eq!(dto.target, actor_url(&alice));
    assert_eq!(
        local_follow_row(&env, &dto.nanoid).await,
        ("accepted".to_string(), true, true, None),
        "stored between the two Local(ProfileId) parties, never as a remote actor URL"
    );
    assert!(
        env.jobs_of(&bob.profile_id).await.is_empty()
            && env.jobs_of(&alice.profile_id).await.is_empty(),
        "no delivery job: the receiving side would refuse an HTTP delivery from a local actor"
    );
    let following = env
        .module
        .list_profile_follows(
            &bob.auth,
            None,
            &bob.nanoid,
            ProfileFollowDirection::Following,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(following.items.len(), 1);
    let followers = env
        .module
        .list_profile_follows(
            &alice.auth,
            None,
            &alice.nanoid,
            ProfileFollowDirection::Followers,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(followers.items.len(), 1, "alice sees bob as a follower");
    assert_eq!(followers.items[0].id, dto.nanoid);
    assert_eq!(
        context(follow(&env, &bob, &actor_url(&alice)).await),
        KernelError::Concurrency,
        "an active follow is not duplicated"
    );

    // unfollow は関係を終了するだけで、配送はない。
    unfollow(&env, &bob, &dto.nanoid).await.unwrap();
    assert_eq!(
        local_follow_row(&env, &dto.nanoid).await,
        ("ended".to_string(), true, true, Some("undone".to_string()))
    );
    assert!(env.jobs_of(&bob.profile_id).await.is_empty());

    // block は双方向の Follow を解除する (配送なし)。carol → alice と alice → carol の両方を作って検証する。
    let incoming = follow(&env, &carol, &actor_url(&alice)).await.unwrap();
    let outgoing = follow(&env, &alice, &actor_url(&carol)).await.unwrap();
    env.module
        .block_actor(&alice.auth, None, &alice.nanoid, &actor_url(&carol))
        .await
        .unwrap();
    for nanoid in [&incoming.nanoid, &outgoing.nanoid] {
        assert_eq!(
            local_follow_row(&env, nanoid).await,
            ("ended".to_string(), true, true, Some("blocked".to_string()))
        );
    }
    assert!(
        env.jobs_of(&alice.profile_id).await.is_empty()
            && env.jobs_of(&carol.profile_id).await.is_empty()
    );
    assert_eq!(
        context(follow(&env, &carol, &actor_url(&alice)).await),
        KernelError::Rejected
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn blocking_ends_both_follow_directions_and_tells_the_peer_with_undo_reject_and_block() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob_url = env.peer_url("/users/bob");
    let outgoing = follow(&env, &alice, &bob_url).await.unwrap();
    deliver_all(&env).await;
    let outgoing_id = {
        let id: i64 = sqlx::query_scalar("SELECT id FROM profile_follows WHERE nanoid = $1")
            .bind(&outgoing.nanoid)
            .fetch_one(&env.iso.pool)
            .await
            .unwrap();
        ProfileFollowId::new(id)
    };
    accept(&env, &outgoing_id).await;
    env.mount_actor("/users/bob").await;

    let (incoming, incoming_activity) = seed_incoming_follow(&env, &alice, &bob_url).await;

    let block = env
        .module
        .block_actor(&alice.auth, None, &alice.nanoid, &bob_url)
        .await
        .unwrap();

    assert_eq!(
        follow_row(&env, &outgoing.nanoid).await,
        ("ended".to_string(), Some("blocked".to_string()))
    );
    let incoming_row: (String, Option<String>) =
        sqlx::query_as("SELECT state, end_reason FROM profile_follows WHERE id = $1")
            .bind(incoming.id().as_ref())
            .fetch_one(&env.iso.pool)
            .await
            .unwrap();
    assert_eq!(
        incoming_row,
        ("ended".to_string(), Some("blocked".to_string()))
    );
    let jobs = env.jobs_of(&alice.profile_id).await;
    let by_type = |t: DeliveryActivityType| -> Vec<&ProfileDeliveryJob> {
        jobs.iter().filter(|job| job.activity_type == t).collect()
    };
    let undo = by_type(DeliveryActivityType::Undo);
    let reject = by_type(DeliveryActivityType::Reject);
    let block_jobs = by_type(DeliveryActivityType::Block);
    assert_eq!((undo.len(), reject.len(), block_jobs.len()), (1, 1, 1));
    assert_eq!(body(block_jobs[0])["type"], "Block");
    assert_eq!(body(block_jobs[0])["object"], bob_url);
    assert_eq!(body(block_jobs[0])["actor"], actor_url(&alice));
    assert_eq!(body(undo[0])["object"]["id"], jobs[0].activity_id.as_str());
    assert_eq!(body(reject[0])["type"], "Reject");
    assert_eq!(body(reject[0])["object"]["id"], incoming_activity.as_str());
    assert_eq!(body(reject[0])["object"]["actor"], bob_url);
    assert_eq!(
        by_type(DeliveryActivityType::Accept)[0].state,
        ProfileDeliveryJobState::Cancelled,
        "an unsent Accept is cancelled"
    );

    // 配送すると peer には Undo(Follow)・Reject(Follow) と Block が届く。
    deliver_all(&env).await;
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
    assert_eq!(
        types,
        vec![
            "Block".to_string(),
            "Reject".to_string(),
            "Undo".to_string()
        ]
    );

    // 二重 block は 409。解除しても関係は復元せず、送った Block だけを Undo(Block) で取り消す。
    assert_eq!(
        context(
            env.module
                .block_actor(&alice.auth, None, &alice.nanoid, &bob_url)
                .await
        ),
        KernelError::Concurrency
    );
    let job_count = env.jobs_of(&alice.profile_id).await.len();
    env.module
        .unblock_actor(&alice.auth, None, &alice.nanoid, &block.nanoid)
        .await
        .unwrap();
    let after_unblock = env.jobs_of(&alice.profile_id).await;
    assert_eq!(
        after_unblock.len(),
        job_count + 1,
        "exactly one Undo(Block)"
    );
    let undo_block = after_unblock.last().unwrap();
    assert_eq!(undo_block.activity_type, DeliveryActivityType::Undo);
    assert_eq!(body(undo_block)["object"]["type"], "Block");
    assert_eq!(body(undo_block)["object"]["id"], body(block_jobs[0])["id"]);
    assert_eq!(
        context(
            env.module
                .unblock_actor(&alice.auth, None, &alice.nanoid, &block.nanoid)
                .await
        ),
        KernelError::NotFound
    );
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), job_count + 1);
    assert_eq!(
        follow_row(&env, &outgoing.nanoid).await.1.as_deref(),
        Some("blocked"),
        "unblocking never restores the detached follow"
    );
    assert_eq!(env.count("SELECT count(*) FROM profile_blocks").await, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn blocking_before_the_follow_was_ever_sent_only_cancels_it() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.mount_actor("/users/bob").await;
    let bob_url = env.peer_url("/users/bob");
    follow(&env, &alice, &bob_url).await.unwrap();

    env.module
        .block_actor(&alice.auth, None, &alice.nanoid, &bob_url)
        .await
        .unwrap();

    let jobs = env.jobs_of(&alice.profile_id).await;
    assert_eq!(jobs.len(), 2, "the unsent Follow and the Block");
    assert_eq!(jobs[0].activity_type, DeliveryActivityType::Follow);
    assert_eq!(jobs[0].state, ProfileDeliveryJobState::Cancelled);
    assert_eq!(jobs[1].activity_type, DeliveryActivityType::Block);
    assert_eq!(jobs[1].state, ProfileDeliveryJobState::Pending);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn muting_changes_no_relationship_and_delivers_nothing() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob_url = env.peer_url("/users/bob");
    let dto = follow(&env, &alice, &bob_url).await.unwrap();
    deliver_all(&env).await;
    let before = env.jobs_of(&alice.profile_id).await.len();

    let mute = env
        .module
        .mute_actor(&alice.auth, None, &alice.nanoid, &bob_url)
        .await
        .unwrap();

    assert_eq!(mute.target, bob_url);
    assert_eq!(follow_row(&env, &dto.nanoid).await.0, "pending");
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), before);
    assert_eq!(
        context(
            env.module
                .mute_actor(&alice.auth, None, &alice.nanoid, &bob_url)
                .await
        ),
        KernelError::Concurrency
    );
    env.module
        .unmute_actor(&alice.auth, None, &alice.nanoid, &mute.nanoid)
        .await
        .unwrap();
    assert_eq!(
        context(
            env.module
                .unmute_actor(&alice.auth, None, &alice.nanoid, &mute.nanoid)
                .await
        ),
        KernelError::NotFound
    );
    assert_eq!(env.count("SELECT count(*) FROM profile_mutes").await, 0);
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), before);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn individual_blocks_do_not_write_domain_controls_and_a_domain_block_refuses_follow() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    // peer は 127.0.0.1 で待ち受けるが、`localhost` 名でも同じ peer に届く。
    let port = env.peer.address().port();
    let target = format!("http://localhost:{port}/users/bob");
    env.mount_actor_as("/users/bob", &target).await;
    {
        let deps = env.module.clone();
        let profile_id = alice.profile_id.clone();
        env.module
            .transaction_manager()
            .transaction(move |executor| {
                Box::pin(async move {
                    deps.domain_control_repository()
                        .create(
                            executor,
                            &profile_id,
                            &ControlDomain::parse("localhost").unwrap(),
                            DomainControlMode::Block,
                        )
                        .await
                        .map(|_| ())
                })
            })
            .await
            .unwrap();
    }

    // domain block が保存されていれば、その domain の actor への Follow は拒否される (D8G)。個別 actor の
    // block の設定は domain 方針を書かず、domain block は個別 block として見えない。
    let refused = follow(&env, &alice, &target).await;
    assert_eq!(context(refused), KernelError::Rejected);
    assert_eq!(env.count("SELECT count(*) FROM profile_follows").await, 0);
    assert!(env.jobs_of(&alice.profile_id).await.is_empty());
    env.module
        .block_actor(&alice.auth, None, &alice.nanoid, &target)
        .await
        .unwrap();

    let controls: (i64, String) =
        sqlx::query_as("SELECT count(*), min(mode) FROM profile_domain_controls")
            .fetch_one(&env.iso.pool)
            .await
            .unwrap();
    assert_eq!(
        controls,
        (1, "block".to_string()),
        "the individual block wrote no domain control"
    );
    // 逆に、domain block は個別 block として見えない。
    assert_eq!(env.count("SELECT count(*) FROM profile_blocks").await, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn invalid_or_unresolvable_targets_create_nothing() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    // 別 actor の文書を返す URL (id 不一致)。
    env.mount_actor_as("/users/impostor", "https://elsewhere.example/users/real")
        .await;
    let impostor = env.peer_url("/users/impostor");
    let missing = env.peer_url("/users/missing");

    for target in [
        "not a url".to_string(),
        "ftp://remote.example/u".to_string(),
        actor_url(&alice),
        format!("{LOCAL_BASE}/ap/profiles/no-such-profile"),
        impostor,
        missing,
    ] {
        assert_eq!(
            context(follow(&env, &alice, &target).await),
            KernelError::Validation,
            "{target}"
        );
    }
    assert_eq!(env.count("SELECT count(*) FROM profile_follows").await, 0);
    assert_eq!(
        env.count("SELECT count(*) FROM profile_delivery_jobs")
            .await,
        0
    );
    assert_eq!(
        context(
            env.module
                .block_actor(&alice.auth, None, &alice.nanoid, &actor_url(&alice))
                .await
        ),
        KernelError::Validation,
        "a Profile cannot block itself"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn following_list_paginates_active_follows_only() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let mut created = Vec::new();
    for name in ["a", "b", "c"] {
        let path = format!("/users/{name}");
        env.mount_actor(&path).await;
        created.push(follow(&env, &alice, &env.peer_url(&path)).await.unwrap());
    }
    unfollow(&env, &alice, &created[1].nanoid).await.unwrap();

    let first = env
        .module
        .list_profile_follows(
            &alice.auth,
            None,
            &alice.nanoid,
            ProfileFollowDirection::Following,
            None,
            Some(1),
        )
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].id, created[0].nanoid);
    let cursor = first.next_cursor.clone().expect("more pages");
    let second = env
        .module
        .list_profile_follows(
            &alice.auth,
            None,
            &alice.nanoid,
            ProfileFollowDirection::Following,
            Some(&cursor),
            Some(5),
        )
        .await
        .unwrap();
    assert_eq!(
        second
            .items
            .iter()
            .map(|i| i.id.clone())
            .collect::<Vec<_>>(),
        vec![created[2].nanoid.clone()],
        "the unfollowed generation is not listed"
    );
    assert!(second.next_cursor.is_none());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn relationship_jobs_never_write_the_legacy_outbox() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let bob_url = env.peer_url("/users/bob");
    let dto = follow(&env, &alice, &bob_url).await.unwrap();
    deliver_all(&env).await;
    unfollow(&env, &alice, &dto.nanoid).await.unwrap();
    seed_incoming_follow(&env, &alice, &bob_url).await;
    env.module
        .block_actor(&alice.auth, None, &alice.nanoid, &bob_url)
        .await
        .unwrap();
    env.module
        .mute_actor(&alice.auth, None, &alice.nanoid, &bob_url)
        .await
        .unwrap();
    deliver_all(&env).await;
    let kinds: Vec<DeliveryActivityType> = env
        .jobs_of(&alice.profile_id)
        .await
        .iter()
        .map(|job| job.activity_type)
        .collect();
    assert!(
        kinds.contains(&DeliveryActivityType::Undo)
            && kinds.contains(&DeliveryActivityType::Reject),
        "the block produced both an Undo(Follow) and a Reject(Follow) delivery job: {kinds:?}"
    );

    // 配送 job は profile_delivery_jobs だけに保存され、旧 Account 単位の outbox_activities には書かない。
    assert_eq!(env.count("SELECT count(*) FROM outbox_activities").await, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_enqueued_activity_id_is_idempotent_across_duplicate_reservations() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let job = NewProfileDeliveryJob::new(
        alice.profile_id.clone(),
        DeliveryActivityType::Follow,
        GraphActivityId::new(format!("{}/activities/dup", actor_url(&alice))).unwrap(),
        GraphInboxUrl::new(env.peer_url("/inbox")).unwrap(),
        "{\"type\":\"Follow\"}",
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let deps = env.module.clone();
    let outcomes = env
        .module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                let first = deps
                    .profile_delivery_enqueuer()
                    .enqueue(executor, &job)
                    .await?;
                let second = deps
                    .profile_delivery_enqueuer()
                    .enqueue(executor, &job)
                    .await?;
                Ok((first, second))
            })
        })
        .await
        .unwrap();
    assert!(outcomes.0.is_newly_enqueued());
    assert!(!outcomes.1.is_newly_enqueued());
    assert_eq!(env.jobs_of(&alice.profile_id).await.len(), 1);
}

fn pair_sql(a: &User, b: &User) -> (i64, i64) {
    (*a.profile_id.as_ref(), *b.profile_id.as_ref())
}

/// block と有効な Follow (または未送信の配送 job) が共存していないこと。
async fn assert_no_follow_survives_the_block(env: &Env, blocker: &User, other: &User) {
    let (a, b) = pair_sql(blocker, other);
    let blocks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM profile_blocks WHERE owner_profile_id = $1 AND target_profile_id = $2",
    )
    .bind(a)
    .bind(b)
    .fetch_one(&env.iso.pool)
    .await
    .unwrap();
    assert_eq!(blocks, 1, "the block is stored");
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM profile_follows WHERE state <> 'ended' AND ((follower_profile_id = $1 AND followee_profile_id = $2) OR (follower_profile_id = $2 AND followee_profile_id = $1))",
    )
    .bind(a)
    .bind(b)
    .fetch_one(&env.iso.pool)
    .await
    .unwrap();
    assert_eq!(active, 0, "no active follow coexists with the block");
    let live_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM profile_delivery_jobs WHERE sender_profile_id IN ($1, $2) AND state IN ('pending', 'in_flight', 'retrying') AND activity_type = 'Follow'",
    )
    .bind(a)
    .bind(b)
    .fetch_one(&env.iso.pool)
    .await
    .unwrap();
    assert_eq!(live_jobs, 0, "no live Follow job coexists with the block");
}

const SLOW_INSERT: &str = "CREATE FUNCTION p3o_slow() RETURNS trigger AS $$ BEGIN PERFORM pg_sleep(0.6); RETURN NEW; END $$ LANGUAGE plpgsql;";

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_follow_racing_a_block_in_the_reverse_direction_never_coexists_with_it() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let bob = env.seed_user().await;
    env.execute(SLOW_INSERT).await;
    env.execute("CREATE TRIGGER p3o_slow_follows BEFORE INSERT ON profile_follows FOR EACH ROW EXECUTE FUNCTION p3o_slow()")
        .await;

    // bob → alice の Follow が (block 判定の後) 保存に時間がかかっている間に、alice が bob を block する。
    let (alice_url, bob_url) = (actor_url(&alice), actor_url(&bob));
    let (follow_result, block_result) = tokio::join!(follow(&env, &bob, &alice_url), async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        env.module
            .block_actor(&alice.auth, None, &alice.nanoid, &bob_url)
            .await
    });

    block_result.unwrap();
    let _ = follow_result;
    assert_no_follow_survives_the_block(&env, &alice, &bob).await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_follow_racing_a_block_by_the_same_profile_never_coexists_with_it() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let bob = env.seed_user().await;
    env.execute(SLOW_INSERT).await;
    env.execute("CREATE TRIGGER p3o_slow_follows BEFORE INSERT ON profile_follows FOR EACH ROW EXECUTE FUNCTION p3o_slow()")
        .await;

    let bob_url = actor_url(&bob);
    let (follow_result, block_result) = tokio::join!(follow(&env, &alice, &bob_url), async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        env.module
            .block_actor(&alice.auth, None, &alice.nanoid, &bob_url)
            .await
    });

    block_result.unwrap();
    let _ = follow_result;
    assert_no_follow_survives_the_block(&env, &alice, &bob).await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_follow_racing_a_slow_block_is_refused_by_it() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let bob = env.seed_user().await;
    env.execute(SLOW_INSERT).await;
    env.execute("CREATE TRIGGER p3o_slow_blocks BEFORE INSERT ON profile_blocks FOR EACH ROW EXECUTE FUNCTION p3o_slow()")
        .await;

    // alice の block の保存中 (未 commit) に bob → alice の Follow が判定を行う。
    let (alice_url, bob_url) = (actor_url(&alice), actor_url(&bob));
    let (block_result, follow_result) = tokio::join!(
        env.module
            .block_actor(&alice.auth, None, &alice.nanoid, &bob_url),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            follow(&env, &bob, &alice_url).await
        }
    );

    block_result.unwrap();
    assert_eq!(
        context(follow_result),
        KernelError::Rejected,
        "the follow waits for the block and is refused by it"
    );
    assert_no_follow_survives_the_block(&env, &alice, &bob).await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn acct_targets_are_resolved_over_webfinger_for_follow_block_and_mute() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let actor = env.mount_actor("/users/bob").await;
    let acct = env.mount_webfinger("bob", &actor).await;

    let dto = follow(&env, &alice, &acct).await.unwrap();
    assert_eq!(
        dto.target, actor,
        "the stored target is the canonical actor URL"
    );
    let job = &env.jobs_of(&alice.profile_id).await[0];
    assert_eq!(job.target_inbox.as_str(), env.peer_url("/inbox"));

    let block = env
        .module
        .block_actor(&alice.auth, None, &alice.nanoid, &acct)
        .await
        .unwrap();
    assert_eq!(block.target, actor);
    let mute = env
        .module
        .mute_actor(
            &alice.auth,
            None,
            &alice.nanoid,
            &format!("@{}", &acct["acct:".len()..]),
        )
        .await
        .unwrap();
    assert_eq!(
        mute.target, actor,
        "the handle also works without the acct: prefix"
    );
    assert_eq!(
        follow_row(&env, &dto.nanoid).await,
        ("ended".to_string(), Some("blocked".to_string()))
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unresolvable_acct_targets_and_unauthorized_callers_are_refused_before_any_effect() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let mallory = env.seed_user().await;
    let domain = env.peer.address().to_string();
    // WebFinger が無い (404)・subject 不一致・ActivityPub の self link なし。
    for handle in [
        format!("acct:ghost@{domain}"),
        "acct:@@".to_string(),
        "bob@".to_string(),
    ] {
        assert_eq!(
            context(follow(&env, &alice, &handle).await),
            KernelError::Validation,
            "{handle}"
        );
    }
    let actor = env.mount_actor("/users/bob").await;
    let acct = env.mount_webfinger("bob", &actor).await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/webfinger"))
        .and(wiremock::matchers::query_param(
            "resource",
            format!("acct:other@{domain}").as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subject": format!("acct:someone-else@{domain}"),
            "links": [{"rel": "self", "type": "application/activity+json", "href": actor}],
        })))
        .mount(&env.peer)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/webfinger"))
        .and(wiremock::matchers::query_param("resource", format!("acct:nolink@{domain}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subject": format!("acct:nolink@{domain}"),
            "links": [{"rel": "http://webfinger.net/rel/profile-page", "href": "https://x.example/"}],
        })))
        .mount(&env.peer)
        .await;
    for handle in [
        format!("acct:other@{domain}"),
        format!("acct:nolink@{domain}"),
    ] {
        assert_eq!(
            context(follow(&env, &alice, &handle).await),
            KernelError::Validation,
            "{handle}"
        );
    }
    assert_eq!(env.count("SELECT count(*) FROM profile_follows").await, 0);

    // 所有者でない呼び出しは、WebFinger へ問い合わせる前に拒否される。
    let before = env.webfinger_requests().await;
    assert_eq!(
        context(
            env.module
                .follow_profile(&mallory.auth, None, &alice.nanoid, &acct)
                .await
        ),
        KernelError::PermissionDenied
    );
    assert_eq!(
        context(
            env.module
                .block_actor(&mallory.auth, None, &alice.nanoid, &acct)
                .await
        ),
        KernelError::PermissionDenied
    );
    assert_eq!(
        env.webfinger_requests().await,
        before,
        "no outbound request for an unauthorized caller"
    );
}
