//! Profile social graph repository の実 DB テスト。テストごとに使い捨ての独立 DB を使う。

use super::profile_delivery::PostgresProfileDeliveryRepository;
use super::profile_follow::PostgresProfileFollowRepository;
use super::profile_graph_test_support::{
    seed_organization_account, seed_personal_profile, seed_profile_for, IsolatedDatabase,
};
use super::profile_inbound_activity::PostgresInboundActivityRepository;
use super::PostgresDatabase;
use kernel::interfaces::database::{DatabaseConnection, TransactionManager};
use kernel::interfaces::repository::{
    BeginFollowOutcome, DependOnInboundActivityRepository, DependOnProfileBlockRepository,
    DependOnProfileDeliveryEnqueuer, DependOnProfileDeliveryJobRepository,
    DependOnProfileFollowRepository, DependOnProfileMuteRepository, InboundActivityRepository,
    ProfileBlockRepository, ProfileDeliveryEnqueuer, ProfileDeliveryJobRepository,
    ProfileFollowRepository, ProfileMuteRepository,
};
use kernel::prelude::entity::{
    ClaimedProfileDeliveryJob, DeliveryActivityType, DeliveryAttemptOutcome, DeliveryClaimPolicy,
    DeliveryFailureReason, DeliveryResultClass, EnqueueOutcome, GraphActivityId, GraphActorUrl,
    GraphInboxUrl, NewInboundActivity, NewProfileDeliveryJob, ProfileBlock, ProfileBlockId,
    ProfileDeliveryJobId, ProfileDeliveryJobState, ProfileFollow, ProfileFollowEndReason,
    ProfileFollowId, ProfileFollowState, ProfileGraphActor, ProfileId, ProfileMute, ProfileMuteId,
    RecordAttemptOutcome,
};
use kernel::KernelError;
use std::collections::HashSet;
use time::{Duration, OffsetDateTime};

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn remote(name: &str) -> ProfileGraphActor {
    ProfileGraphActor::Remote(
        GraphActorUrl::new(format!("https://remote.example/users/{name}")).unwrap(),
    )
}

fn local(id: &ProfileId) -> ProfileGraphActor {
    ProfileGraphActor::Local(id.clone())
}

fn activity(path: &str) -> GraphActivityId {
    GraphActivityId::new(format!("https://activities.example/{path}")).unwrap()
}

fn pending(follower: &ProfileGraphActor, followee: &ProfileGraphActor, act: &str) -> ProfileFollow {
    ProfileFollow::new_pending(
        ProfileFollowId::new(kernel::generate_id()),
        follower.clone(),
        followee.clone(),
        activity(act),
        now(),
    )
    .unwrap()
}

fn accepted(
    follower: &ProfileGraphActor,
    followee: &ProfileGraphActor,
    act: &str,
) -> ProfileFollow {
    ProfileFollow::new_accepted(
        ProfileFollowId::new(kernel::generate_id()),
        follower.clone(),
        followee.clone(),
        activity(act),
        now(),
    )
    .unwrap()
}

async fn begin(db: &PostgresDatabase, follow: &ProfileFollow) -> BeginFollowOutcome {
    let mut conn = db.connection().await.unwrap();
    db.profile_follow_repository()
        .begin_generation(&mut conn, follow)
        .await
        .unwrap()
}

fn job(sender: &ProfileId, act: &str, inbox: &str) -> NewProfileDeliveryJob {
    NewProfileDeliveryJob::new(
        sender.clone(),
        DeliveryActivityType::Follow,
        activity(act),
        GraphInboxUrl::new(format!("https://remote.example/{inbox}/inbox")).unwrap(),
        format!("{{\"type\":\"Follow\",\"id\":\"https://activities.example/{act}\"}}"),
        now(),
    )
    .unwrap()
}

fn policy(limit: i64) -> DeliveryClaimPolicy {
    DeliveryClaimPolicy {
        lease_duration: Duration::seconds(60),
        limit,
        max_attempts: 5,
    }
}

async fn enqueue(db: &PostgresDatabase, job: &NewProfileDeliveryJob) -> EnqueueOutcome {
    let mut conn = db.connection().await.unwrap();
    db.profile_delivery_enqueuer()
        .enqueue(&mut conn, job)
        .await
        .unwrap()
}

async fn claim(
    db: &PostgresDatabase,
    worker: &str,
    at: OffsetDateTime,
    limit: i64,
) -> Vec<ClaimedProfileDeliveryJob> {
    let mut conn = db.connection().await.unwrap();
    db.profile_delivery_job_repository()
        .claim_due(&mut conn, worker, at, &policy(limit))
        .await
        .unwrap()
}

async fn record(
    db: &PostgresDatabase,
    claim: &ClaimedProfileDeliveryJob,
    at: OffsetDateTime,
    outcome: &DeliveryAttemptOutcome,
) -> RecordAttemptOutcome {
    let mut conn = db.connection().await.unwrap();
    db.profile_delivery_job_repository()
        .record_attempt(&mut conn, claim, at, outcome)
        .await
        .unwrap()
}

async fn job_state(db: &PostgresDatabase, id: &ProfileDeliveryJobId) -> ProfileDeliveryJobState {
    let mut conn = db.connection().await.unwrap();
    db.profile_delivery_job_repository()
        .find_by_id(&mut conn, id)
        .await
        .unwrap()
        .unwrap()
        .state
}

mod follow {
    use super::*;

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn relations_are_separated_per_profile_and_account() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let org = seed_organization_account(db).await;
        let p1 = seed_profile_for(db, &org).await;
        let p2 = seed_profile_for(db, &org).await;
        let (_, p3) = seed_personal_profile(db).await;
        let bob = remote("bob");

        for (profile, act) in [(&p1, "a1"), (&p2, "a2"), (&p3, "a3")] {
            let outcome = begin(db, &pending(&local(profile), &bob, act)).await;
            assert_eq!(outcome, BeginFollowOutcome::Created);
        }
        // 受信側 (リモート → ローカル) も Profile ごとに分離される。
        assert_eq!(
            begin(db, &accepted(&bob, &local(&p1), "in1")).await,
            BeginFollowOutcome::Created
        );

        let mut conn = db.connection().await.unwrap();
        let repo = db.profile_follow_repository();
        for profile in [&p1, &p2, &p3] {
            let following = repo
                .list_following(&mut conn, profile, None, 10)
                .await
                .unwrap();
            assert_eq!(following.len(), 1, "each profile follows exactly bob once");
            assert_eq!(following[0].follower(), &local(profile));
        }
        let followers_p1 = repo.list_followers(&mut conn, &p1, None, 10).await.unwrap();
        assert_eq!(followers_p1.len(), 1);
        assert_eq!(followers_p1[0].follower(), &bob);
        assert!(repo
            .list_followers(&mut conn, &p2, None, 10)
            .await
            .unwrap()
            .is_empty());
        assert!(repo
            .list_followers(&mut conn, &p3, None, 10)
            .await
            .unwrap()
            .is_empty());

        let a = repo
            .find_active(&mut conn, &local(&p1), &bob)
            .await
            .unwrap()
            .unwrap();
        let b = repo
            .find_active(&mut conn, &local(&p2), &bob)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(a.id(), b.id());

        // Local → Local の関係も Profile 単位 (同一 Account 内の別 Profile 同士も可)。
        assert_eq!(
            begin(db, &pending(&local(&p1), &local(&p2), "ll")).await,
            BeginFollowOutcome::Created
        );
        assert_eq!(
            repo.list_followers(&mut conn, &p2, None, 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            repo.list_following(&mut conn, &p1, None, 10)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn refollow_is_a_new_generation_and_old_activity_does_not_resurrect() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let bob = remote("bob");
        let me = local(&p1);

        let gen1 = pending(&me, &bob, "follow-1");
        assert_eq!(begin(db, &gen1).await, BeginFollowOutcome::Created);

        let mut conn = db.connection().await.unwrap();
        let repo = db.profile_follow_repository();
        assert!(repo.accept(&mut conn, gen1.id(), now()).await.unwrap());
        assert!(repo
            .end(&mut conn, gen1.id(), ProfileFollowEndReason::Undone, now())
            .await
            .unwrap());
        drop(conn);

        let gen2 = pending(&me, &bob, "follow-2");
        assert_eq!(begin(db, &gen2).await, BeginFollowOutcome::Created);

        let mut conn = db.connection().await.unwrap();
        let generations = repo.list_generations(&mut conn, &me, &bob).await.unwrap();
        assert_eq!(generations.len(), 2);
        assert_eq!(generations[0].activity_id(), &activity("follow-1"));
        assert_eq!(generations[0].state(), &ProfileFollowState::Ended);
        assert_eq!(
            generations[0].end_reason(),
            &Some(ProfileFollowEndReason::Undone)
        );
        assert!(generations[0].accepted_at().is_some());
        assert_eq!(generations[1].activity_id(), &activity("follow-2"));
        assert_eq!(generations[1].state(), &ProfileFollowState::Pending);
        drop(conn);

        // 古い Follow activity の再送は、世代を作らず終了済みの世代を返す。
        let replay = pending(&me, &bob, "follow-1");
        match begin(db, &replay).await {
            BeginFollowOutcome::ReplayedActivity(existing) => {
                assert_eq!(existing.id(), gen1.id());
                assert_eq!(existing.state(), &ProfileFollowState::Ended);
            }
            other => panic!("expected replay, got {other:?}"),
        }
        let mut conn = db.connection().await.unwrap();
        let active = repo
            .find_active(&mut conn, &me, &bob)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.id(), gen2.id());
        assert_eq!(
            repo.list_generations(&mut conn, &me, &bob)
                .await
                .unwrap()
                .len(),
            2
        );

        // gen2 も終了した後でも、古い activity では復活しない。
        assert!(repo
            .end(
                &mut conn,
                gen2.id(),
                ProfileFollowEndReason::Rejected,
                now()
            )
            .await
            .unwrap());
        drop(conn);
        assert!(matches!(
            begin(db, &pending(&me, &bob, "follow-1")).await,
            BeginFollowOutcome::ReplayedActivity(_)
        ));
        let mut conn = db.connection().await.unwrap();
        assert!(repo
            .find_active(&mut conn, &me, &bob)
            .await
            .unwrap()
            .is_none());

        // 元 activity ID で世代を引ける。
        let found = repo
            .find_by_follower_activity(&mut conn, &me, &activity("follow-2"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id(), gen2.id());
        let by_nanoid = repo
            .find_by_nanoid(&mut conn, gen2.nanoid())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(by_nanoid.id(), gen2.id());
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn second_active_generation_with_other_activity_is_refused() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let bob = remote("bob");
        let gen1 = pending(&local(&p1), &bob, "f1");
        assert_eq!(begin(db, &gen1).await, BeginFollowOutcome::Created);
        match begin(db, &pending(&local(&p1), &bob, "f2")).await {
            BeginFollowOutcome::ActiveGenerationExists(existing) => {
                assert_eq!(existing.id(), gen1.id())
            }
            other => panic!("expected ActiveGenerationExists, got {other:?}"),
        }
        let mut conn = db.connection().await.unwrap();
        let all = db
            .profile_follow_repository()
            .list_generations(&mut conn, &local(&p1), &bob)
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn concurrent_generation_creation_leaves_one_active_generation() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let bob = remote("bob");

        let mut tasks = Vec::new();
        for n in 0..16 {
            let db = db.clone();
            let follow = pending(&local(&p1), &bob, &format!("race-{n}"));
            tasks.push(tokio::spawn(async move { begin(&db, &follow).await }));
        }
        let mut created = 0;
        let mut refused = 0;
        for task in tasks {
            match task.await.unwrap() {
                BeginFollowOutcome::Created => created += 1,
                BeginFollowOutcome::ActiveGenerationExists(_) => refused += 1,
                other => panic!("unexpected outcome {other:?}"),
            }
        }
        assert_eq!((created, refused), (1, 15));

        // 同じ activity ID の同時再送は 1 つだけ作られ、残りは再送として扱われる。
        let alice = remote("alice");
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let db = db.clone();
            let follow = pending(&local(&p1), &alice, "same-activity");
            tasks.push(tokio::spawn(async move { begin(&db, &follow).await }));
        }
        let mut created = 0;
        let mut replayed = 0;
        for task in tasks {
            match task.await.unwrap() {
                BeginFollowOutcome::Created => created += 1,
                BeginFollowOutcome::ReplayedActivity(_) => replayed += 1,
                other => panic!("unexpected outcome {other:?}"),
            }
        }
        assert_eq!((created, replayed), (1, 15));

        let mut conn = db.connection().await.unwrap();
        let repo = db.profile_follow_repository();
        assert_eq!(
            repo.list_generations(&mut conn, &local(&p1), &bob)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            repo.list_generations(&mut conn, &local(&p1), &alice)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn state_transitions_are_compare_and_swap() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let follow = pending(&remote("bob"), &local(&p1), "in");
        begin(db, &follow).await;
        let mut conn = db.connection().await.unwrap();
        let repo = db.profile_follow_repository();

        assert!(repo.accept(&mut conn, follow.id(), now()).await.unwrap());
        assert!(!repo.accept(&mut conn, follow.id(), now()).await.unwrap());
        let reloaded = repo
            .find_by_id(&mut conn, follow.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.state(), &ProfileFollowState::Accepted);
        assert!(reloaded.accepted_at().is_some());

        assert!(repo
            .end(
                &mut conn,
                follow.id(),
                ProfileFollowEndReason::Rejected,
                now()
            )
            .await
            .unwrap());
        assert!(!repo
            .end(
                &mut conn,
                follow.id(),
                ProfileFollowEndReason::Undone,
                now()
            )
            .await
            .unwrap());
        assert!(!repo.accept(&mut conn, follow.id(), now()).await.unwrap());
        let ended = repo
            .find_by_id(&mut conn, follow.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ended.end_reason(), &Some(ProfileFollowEndReason::Rejected));
        assert!(repo
            .list_followers(&mut conn, &p1, None, 10)
            .await
            .unwrap()
            .is_empty());
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn end_active_between_detaches_both_directions_only() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let (_, p2) = seed_personal_profile(db).await;
        let bob = remote("bob");
        let outbound = accepted(&local(&p1), &bob, "out");
        let inbound = pending(&bob, &local(&p1), "in");
        let unrelated = accepted(&local(&p1), &local(&p2), "ll");
        let other_profile = accepted(&local(&p2), &bob, "other");
        for follow in [&outbound, &inbound, &unrelated, &other_profile] {
            assert_eq!(begin(db, follow).await, BeginFollowOutcome::Created);
        }

        let mut conn = db.connection().await.unwrap();
        let repo = db.profile_follow_repository();
        let ended = repo
            .end_active_between(
                &mut conn,
                &local(&p1),
                &bob,
                ProfileFollowEndReason::Blocked,
                now(),
            )
            .await
            .unwrap();
        assert_eq!(ended.len(), 2);
        assert!(ended
            .iter()
            .all(|f| f.state() == &ProfileFollowState::Ended));
        assert!(ended
            .iter()
            .all(|f| f.end_reason() == &Some(ProfileFollowEndReason::Blocked)));
        let accepted_one = ended.iter().find(|f| f.id() == outbound.id()).unwrap();
        assert!(accepted_one.accepted_at().is_some());
        let pending_one = ended.iter().find(|f| f.id() == inbound.id()).unwrap();
        assert!(pending_one.accepted_at().is_none());

        assert!(repo
            .find_active(&mut conn, &local(&p1), &local(&p2))
            .await
            .unwrap()
            .is_some());
        assert!(repo
            .find_active(&mut conn, &local(&p2), &bob)
            .await
            .unwrap()
            .is_some());
        assert!(repo
            .end_active_between(
                &mut conn,
                &local(&p1),
                &bob,
                ProfileFollowEndReason::Blocked,
                now()
            )
            .await
            .unwrap()
            .is_empty());
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn lists_are_paginated_by_id() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let mut ids = Vec::new();
        for n in 0..5 {
            let follow = pending(&local(&p1), &remote(&format!("u{n}")), &format!("f{n}"));
            begin(db, &follow).await;
            ids.push(follow.id().clone());
        }
        let mut conn = db.connection().await.unwrap();
        let repo = db.profile_follow_repository();
        let first = repo.list_following(&mut conn, &p1, None, 2).await.unwrap();
        assert_eq!(
            first.iter().map(|f| f.id().clone()).collect::<Vec<_>>(),
            ids[..2]
        );
        let second = repo
            .list_following(&mut conn, &p1, Some(first[1].id()), 2)
            .await
            .unwrap();
        assert_eq!(
            second.iter().map(|f| f.id().clone()).collect::<Vec<_>>(),
            ids[2..4]
        );
        let third = repo
            .list_following(&mut conn, &p1, Some(second[1].id()), 2)
            .await
            .unwrap();
        assert_eq!(third.len(), 1);
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn schema_rejects_invalid_rows() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let (_, p1) = seed_personal_profile(&iso.database).await;
        let p1 = *p1.as_ref();
        let insert = |extra: &'static str,
                      binds: (
            Option<i64>,
            Option<&'static str>,
            Option<i64>,
            Option<&'static str>,
        )| {
            let pool = iso.pool.clone();
            async move {
                sqlx::query(&format!(
                    "INSERT INTO profile_follows (id, nanoid, follower_profile_id, follower_actor_url, \
                     followee_profile_id, followee_actor_url, activity_id, state, {extra}, created_at, updated_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, 'https://a.example/1', 'pending', NULL, now(), now())"
                ))
                .bind(kernel::generate_id())
                .bind(nanoid::nanoid!())
                .bind(binds.0)
                .bind(binds.1)
                .bind(binds.2)
                .bind(binds.3)
                .execute(&pool)
                .await
            }
        };
        // remote → remote
        assert!(insert(
            "end_reason",
            (
                None,
                Some("https://r.example/a"),
                None,
                Some("https://r.example/b")
            )
        )
        .await
        .is_err());
        // self follow
        assert!(insert("end_reason", (Some(p1), None, Some(p1), None))
            .await
            .is_err());
        // 両方 NULL
        assert!(insert("end_reason", (None, None, Some(p1), None))
            .await
            .is_err());
        // 正常
        assert!(insert(
            "end_reason",
            (Some(p1), None, None, Some("https://r.example/b"))
        )
        .await
        .is_ok());
        // pending なのに ended_at がある
        let bad_ended = sqlx::query(
            "UPDATE profile_follows SET ended_at = now() WHERE follower_profile_id = $1",
        )
        .bind(p1)
        .execute(&iso.pool)
        .await;
        assert!(bad_ended.is_err());
    }
}

mod relation {
    use super::*;

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn blocks_are_scoped_to_the_owner_profile() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let org = seed_organization_account(db).await;
        let p1 = seed_profile_for(db, &org).await;
        let p2 = seed_profile_for(db, &org).await;
        let (_, p3) = seed_personal_profile(db).await;
        let bob = remote("bob");
        let mut conn = db.connection().await.unwrap();
        let repo = db.profile_block_repository();

        let block = ProfileBlock::new(
            ProfileBlockId::new(kernel::generate_id()),
            p1.clone(),
            bob.clone(),
            now(),
        )
        .unwrap();
        assert!(repo.insert_if_absent(&mut conn, &block).await.unwrap());
        let duplicate = ProfileBlock::new(
            ProfileBlockId::new(kernel::generate_id()),
            p1.clone(),
            bob.clone(),
            now(),
        )
        .unwrap();
        assert!(!repo.insert_if_absent(&mut conn, &duplicate).await.unwrap());

        assert!(repo.is_blocked(&mut conn, &p1, &bob).await.unwrap());
        assert!(!repo.is_blocked(&mut conn, &p2, &bob).await.unwrap());
        assert!(!repo.is_blocked(&mut conn, &p3, &bob).await.unwrap());
        // 向きがある: bob 側 (remote) は owner になれず、p1 → p2 の block もない。
        assert!(!repo.is_blocked(&mut conn, &p1, &local(&p2)).await.unwrap());

        let local_block = ProfileBlock::new(
            ProfileBlockId::new(kernel::generate_id()),
            p2.clone(),
            local(&p3),
            now(),
        )
        .unwrap();
        assert!(repo
            .insert_if_absent(&mut conn, &local_block)
            .await
            .unwrap());
        assert!(repo.is_blocked(&mut conn, &p2, &local(&p3)).await.unwrap());
        assert!(!repo.is_blocked(&mut conn, &p3, &local(&p2)).await.unwrap());

        let listed = repo.list(&mut conn, &p1, None, 10).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].target(), &bob);
        assert_eq!(listed[0].nanoid(), block.nanoid());

        // 他 Profile の nanoid では消せない。
        assert!(!repo
            .delete_by_nanoid(&mut conn, &p2, block.nanoid())
            .await
            .unwrap());
        assert!(repo.is_blocked(&mut conn, &p1, &bob).await.unwrap());
        assert!(repo
            .delete_by_nanoid(&mut conn, &p1, block.nanoid())
            .await
            .unwrap());
        assert!(!repo.is_blocked(&mut conn, &p1, &bob).await.unwrap());
        assert!(!repo
            .delete_by_nanoid(&mut conn, &p1, block.nanoid())
            .await
            .unwrap());
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn mutes_are_scoped_and_independent_of_blocks_and_follows() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let (_, p2) = seed_personal_profile(db).await;
        let bob = remote("bob");
        begin(db, &accepted(&local(&p1), &bob, "f")).await;
        let mut conn = db.connection().await.unwrap();
        let mutes = db.profile_mute_repository();

        let mute = ProfileMute::new(
            ProfileMuteId::new(kernel::generate_id()),
            p1.clone(),
            bob.clone(),
            now(),
        )
        .unwrap();
        assert!(mutes.insert_if_absent(&mut conn, &mute).await.unwrap());
        let again = ProfileMute::new(
            ProfileMuteId::new(kernel::generate_id()),
            p1.clone(),
            bob.clone(),
            now(),
        )
        .unwrap();
        assert!(!mutes.insert_if_absent(&mut conn, &again).await.unwrap());
        assert!(mutes.is_muted(&mut conn, &p1, &bob).await.unwrap());
        assert!(!mutes.is_muted(&mut conn, &p2, &bob).await.unwrap());
        // mute は block ではなく、Follow も変えない。
        assert!(!db
            .profile_block_repository()
            .is_blocked(&mut conn, &p1, &bob)
            .await
            .unwrap());
        let follow = db
            .profile_follow_repository()
            .find_active(&mut conn, &local(&p1), &bob)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(follow.state(), &ProfileFollowState::Accepted);

        assert_eq!(mutes.list(&mut conn, &p1, None, 10).await.unwrap().len(), 1);
        assert!(!mutes
            .delete_by_nanoid(&mut conn, &p2, mute.nanoid())
            .await
            .unwrap());
        assert!(mutes
            .delete_by_nanoid(&mut conn, &p1, mute.nanoid())
            .await
            .unwrap());
        assert!(!mutes.is_muted(&mut conn, &p1, &bob).await.unwrap());
    }
}

mod inbound {
    use super::*;

    fn received(receiver: &ProfileId, actor: &str, act: &str) -> NewInboundActivity {
        NewInboundActivity::new(
            receiver.clone(),
            GraphActorUrl::new(format!("https://remote.example/users/{actor}")).unwrap(),
            activity(act),
            "Follow",
            now(),
        )
        .unwrap()
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn duplicate_receipts_are_detected_and_counted() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let (_, p2) = seed_personal_profile(db).await;
        let mut conn = db.connection().await.unwrap();
        let repo = db.inbound_activity_repository();

        let first = repo
            .record(&mut conn, &received(&p1, "bob", "a1"))
            .await
            .unwrap();
        assert!(first.first_seen);
        assert_eq!(first.receive_count, 1);
        let second = repo
            .record(&mut conn, &received(&p1, "bob", "a1"))
            .await
            .unwrap();
        assert!(!second.first_seen);
        assert_eq!(second.receive_count, 2);

        // actor・受信 Profile・activity ID のどれかが違えば別の記録。
        assert!(
            repo.record(&mut conn, &received(&p1, "mallory", "a1"))
                .await
                .unwrap()
                .first_seen
        );
        assert!(
            repo.record(&mut conn, &received(&p2, "bob", "a1"))
                .await
                .unwrap()
                .first_seen
        );
        assert!(
            repo.record(&mut conn, &received(&p1, "bob", "a2"))
                .await
                .unwrap()
                .first_seen
        );

        let record = repo
            .find(
                &mut conn,
                &p1,
                &GraphActorUrl::new("https://remote.example/users/bob").unwrap(),
                &activity("a1"),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.receive_count, 2);
        assert_eq!(record.activity_type, "Follow");
        assert!(record.last_received_at >= record.first_received_at);
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn concurrent_duplicate_receipts_have_one_first_seen() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let db = db.clone();
            let activity = received(&p1, "bob", "race");
            tasks.push(tokio::spawn(async move {
                let mut conn = db.connection().await.unwrap();
                db.inbound_activity_repository()
                    .record(&mut conn, &activity)
                    .await
                    .unwrap()
            }));
        }
        let mut firsts = 0;
        let mut counts = HashSet::new();
        for task in tasks {
            let receipt = task.await.unwrap();
            firsts += usize::from(receipt.first_seen);
            counts.insert(receipt.receive_count);
        }
        assert_eq!(firsts, 1);
        assert_eq!(counts.len(), 16, "every receipt gets a distinct count");
        assert_eq!(counts.iter().max(), Some(&16));
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn rollback_forgets_the_receipt_and_purge_removes_old_records() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let activity_a = received(&p1, "bob", "a1");

        let result: error_stack::Result<(), KernelError> = db
            .transaction(|conn| {
                let activity_a = activity_a.clone();
                Box::pin(async move {
                    let receipt = PostgresInboundActivityRepository
                        .record(conn, &activity_a)
                        .await?;
                    assert!(receipt.first_seen);
                    Err(error_stack::Report::new(KernelError::Internal))
                })
            })
            .await;
        assert!(result.is_err());
        let mut conn = db.connection().await.unwrap();
        let repo = db.inbound_activity_repository();
        assert!(
            repo.record(&mut conn, &activity_a)
                .await
                .unwrap()
                .first_seen
        );

        let old = NewInboundActivity::new(
            p1.clone(),
            GraphActorUrl::new("https://remote.example/users/old").unwrap(),
            activity("old"),
            "Undo",
            now() - Duration::days(40),
        )
        .unwrap();
        repo.record(&mut conn, &old).await.unwrap();
        let purged = repo
            .purge_received_before(&mut conn, now() - Duration::days(30))
            .await
            .unwrap();
        assert_eq!(purged, 1);
        assert!(repo.record(&mut conn, &old).await.unwrap().first_seen);
    }
}

mod delivery {
    use super::*;

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn enqueue_is_idempotent_and_keeps_payload_bytes() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let (_, p2) = seed_personal_profile(db).await;

        let first = job(&p1, "a1", "bob");
        let id = match enqueue(db, &first).await {
            EnqueueOutcome::Enqueued(id) => id,
            other => panic!("expected Enqueued, got {other:?}"),
        };
        assert_eq!(id, first.id);
        let again = job(&p1, "a1", "bob");
        assert_eq!(
            enqueue(db, &again).await,
            EnqueueOutcome::AlreadyEnqueued(id.clone())
        );
        // 宛先・送信 Profile・activity のどれかが違えば別 job。
        assert!(enqueue(db, &job(&p1, "a1", "alice"))
            .await
            .is_newly_enqueued());
        assert!(enqueue(db, &job(&p2, "a1", "bob"))
            .await
            .is_newly_enqueued());
        assert!(enqueue(db, &job(&p1, "a2", "bob"))
            .await
            .is_newly_enqueued());

        let mut conn = db.connection().await.unwrap();
        let stored = db
            .profile_delivery_job_repository()
            .find_by_key(&mut conn, &p1, &first.activity_id, &first.target_inbox)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.payload, first.payload);
        assert_eq!(stored.state, ProfileDeliveryJobState::Pending);
        assert_eq!(stored.attempt_count, 0);
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn concurrent_enqueue_of_same_key_creates_one_job() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let db = db.clone();
            let job = job(&p1, "race", "bob");
            tasks.push(tokio::spawn(async move { enqueue(&db, &job).await }));
        }
        let mut newly = 0;
        let mut ids = HashSet::new();
        for task in tasks {
            let outcome = task.await.unwrap();
            newly += usize::from(outcome.is_newly_enqueued());
            ids.insert(*outcome.job_id().as_ref());
        }
        assert_eq!(newly, 1);
        assert_eq!(ids.len(), 1);
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn relation_change_and_enqueue_commit_or_roll_back_together() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let bob = remote("bob");
        let me = local(&p1);

        // rollback: どちらも残らない。
        let follow = pending(&me, &bob, "f-rollback");
        let rollback_job = job(&p1, "f-rollback", "bob").with_follow(follow.id().clone());
        let result: error_stack::Result<(), KernelError> = db
            .transaction(|conn| {
                let follow = follow.clone();
                let rollback_job = rollback_job.clone();
                Box::pin(async move {
                    let outcome = PostgresProfileFollowRepository
                        .begin_generation(conn, &follow)
                        .await?;
                    assert_eq!(outcome, BeginFollowOutcome::Created);
                    PostgresProfileDeliveryRepository
                        .enqueue(conn, &rollback_job)
                        .await?;
                    Err(error_stack::Report::new(KernelError::Internal))
                })
            })
            .await;
        assert!(result.is_err());
        let mut conn = db.connection().await.unwrap();
        assert!(db
            .profile_follow_repository()
            .find_by_id(&mut conn, follow.id())
            .await
            .unwrap()
            .is_none());
        assert!(db
            .profile_delivery_job_repository()
            .find_by_id(&mut conn, &rollback_job.id)
            .await
            .unwrap()
            .is_none());
        drop(conn);

        // commit: 両方残り、job は follow に紐づく。
        let follow = pending(&me, &bob, "f-commit");
        let commit_job = job(&p1, "f-commit", "bob").with_follow(follow.id().clone());
        db.transaction(|conn| {
            let follow = follow.clone();
            let commit_job = commit_job.clone();
            Box::pin(async move {
                PostgresProfileFollowRepository
                    .begin_generation(conn, &follow)
                    .await?;
                PostgresProfileDeliveryRepository
                    .enqueue(conn, &commit_job)
                    .await?;
                Ok(())
            })
        })
        .await
        .unwrap();
        let mut conn = db.connection().await.unwrap();
        assert!(db
            .profile_follow_repository()
            .find_by_id(&mut conn, follow.id())
            .await
            .unwrap()
            .is_some());
        let stored = db
            .profile_delivery_job_repository()
            .find_by_id(&mut conn, &commit_job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.follow_id.as_ref(), Some(follow.id()));
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn lease_prevents_double_claim_and_expiry_hands_over() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let new_job = job(&p1, "a1", "bob");
        enqueue(db, &new_job).await;
        let t0 = now();

        let first = claim(db, "worker-a", t0, 10).await;
        assert_eq!(first.len(), 1);
        let first = &first[0];
        assert_eq!(first.attempt_no, 1);
        assert_eq!(first.lease_owner, "worker-a");
        assert_eq!(first.payload, new_job.payload);
        // lease 中は別 worker も同じ worker も取得できない。
        assert!(claim(db, "worker-b", t0 + Duration::seconds(30), 10)
            .await
            .is_empty());
        assert!(claim(db, "worker-a", t0 + Duration::seconds(30), 10)
            .await
            .is_empty());

        // 期限切れ: 別 worker が回収する。前回の試行は lease_expired として履歴に残る。
        let t1 = t0 + Duration::seconds(61);
        let second = claim(db, "worker-b", t1, 10).await;
        assert_eq!(second.len(), 1);
        let second = &second[0];
        assert_eq!(second.attempt_no, 2);
        assert_ne!(second.lease_token, first.lease_token);

        // 古い worker の結果は拒否され、状態を変えない。
        let late = record(
            db,
            first,
            t1,
            &DeliveryAttemptOutcome::Delivered {
                http_status: Some(202),
            },
        )
        .await;
        assert_eq!(late, RecordAttemptOutcome::LeaseLost);
        assert_eq!(
            job_state(db, &new_job.id).await,
            ProfileDeliveryJobState::InFlight
        );

        let done = record(
            db,
            second,
            t1,
            &DeliveryAttemptOutcome::Delivered {
                http_status: Some(202),
            },
        )
        .await;
        assert_eq!(done, RecordAttemptOutcome::Recorded);

        let mut conn = db.connection().await.unwrap();
        let repo = db.profile_delivery_job_repository();
        let stored = repo
            .find_by_id(&mut conn, &new_job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, ProfileDeliveryJobState::Delivered);
        assert_eq!(stored.attempt_count, 2);
        assert_eq!(stored.last_result, Some(DeliveryResultClass::Delivered));
        assert_eq!(stored.last_http_status, Some(202));
        assert!(stored.delivered_at.is_some());
        assert!(stored.lease_owner.is_none());
        let attempts = repo.list_attempts(&mut conn, &new_job.id).await.unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].result, DeliveryResultClass::LeaseExpired);
        assert_eq!(attempts[1].result, DeliveryResultClass::Delivered);
        // 配送済みは再取得されない。
        drop(conn);
        assert!(claim(db, "worker-c", t1 + Duration::days(1), 10)
            .await
            .is_empty());
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn retry_failed_and_not_before_are_respected() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let t0 = now();
        let delayed = job(&p1, "delayed", "bob").not_before(t0 + Duration::minutes(10));
        let retrying = job(&p1, "retry", "bob").not_before(t0);
        let failing = job(&p1, "fail", "bob").not_before(t0);
        for j in [&delayed, &retrying, &failing] {
            enqueue(db, j).await;
        }

        let claimed = claim(db, "w", t0, 10).await;
        assert_eq!(claimed.len(), 2, "not_before keeps the delayed job out");
        for c in &claimed {
            let outcome = if c.id == retrying.id {
                DeliveryAttemptOutcome::Retry {
                    next_attempt_at: t0 + Duration::minutes(5),
                    result: DeliveryResultClass::HttpServerError,
                    http_status: Some(503),
                }
            } else {
                DeliveryAttemptOutcome::Failed {
                    reason: DeliveryFailureReason::PermanentError,
                    result: DeliveryResultClass::HttpClientError,
                    http_status: Some(410),
                }
            };
            assert_eq!(
                record(db, c, t0, &outcome).await,
                RecordAttemptOutcome::Recorded
            );
        }
        assert_eq!(
            job_state(db, &retrying.id).await,
            ProfileDeliveryJobState::Retrying
        );
        assert_eq!(
            job_state(db, &failing.id).await,
            ProfileDeliveryJobState::Failed
        );

        assert!(claim(db, "w", t0 + Duration::minutes(1), 10)
            .await
            .is_empty());
        let at5 = claim(db, "w", t0 + Duration::minutes(5), 10).await;
        assert_eq!(at5.len(), 1);
        assert_eq!(at5[0].id, retrying.id);
        assert_eq!(at5[0].attempt_no, 2);
        let delivered = DeliveryAttemptOutcome::Delivered {
            http_status: Some(200),
        };
        assert_eq!(
            record(db, &at5[0], t0 + Duration::minutes(5), &delivered).await,
            RecordAttemptOutcome::Recorded
        );
        let at10 = claim(db, "w", t0 + Duration::minutes(10), 10).await;
        assert_eq!(at10.len(), 1);
        assert_eq!(at10[0].id, delayed.id);

        let mut conn = db.connection().await.unwrap();
        let repo = db.profile_delivery_job_repository();
        let failed = repo
            .find_by_id(&mut conn, &failing.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            failed.failure_reason,
            Some(DeliveryFailureReason::PermanentError)
        );
        assert_eq!(failed.last_http_status, Some(410));
        let attempts = repo.list_attempts(&mut conn, &retrying.id).await.unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].result, DeliveryResultClass::HttpServerError);
        assert_eq!(attempts[0].http_status, Some(503));
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn abandoned_leases_stop_after_max_attempts() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let crashing = job(&p1, "crash", "bob");
        enqueue(db, &crashing).await;
        let mut t = now();
        let tight = DeliveryClaimPolicy {
            lease_duration: Duration::seconds(10),
            limit: 10,
            max_attempts: 2,
        };
        for expected_attempt in 1..=2 {
            let mut conn = db.connection().await.unwrap();
            let claimed = db
                .profile_delivery_job_repository()
                .claim_due(&mut conn, "crashy", t, &tight)
                .await
                .unwrap();
            assert_eq!(claimed.len(), 1);
            assert_eq!(claimed[0].attempt_no, expected_attempt);
            t += Duration::seconds(11);
        }
        let mut conn = db.connection().await.unwrap();
        let repo = db.profile_delivery_job_repository();
        assert!(repo
            .claim_due(&mut conn, "crashy", t, &tight)
            .await
            .unwrap()
            .is_empty());
        let stored = repo
            .find_by_id(&mut conn, &crashing.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, ProfileDeliveryJobState::Failed);
        assert_eq!(
            stored.failure_reason,
            Some(DeliveryFailureReason::RetriesExhausted)
        );
        assert_eq!(stored.last_result, Some(DeliveryResultClass::LeaseExpired));
        assert_eq!(
            repo.list_attempts(&mut conn, &crashing.id)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn cancel_removes_undelivered_jobs_and_keeps_history_of_in_flight() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let bob = remote("bob");
        let follow = pending(&local(&p1), &bob, "f1");
        begin(db, &follow).await;
        let other_follow = pending(&local(&p1), &remote("alice"), "f2");
        begin(db, &other_follow).await;

        let follow_job = job(&p1, "f1", "bob").with_follow(follow.id().clone());
        let mut undo_job = job(&p1, "undo-f1", "bob").with_follow(follow.id().clone());
        undo_job.activity_type = DeliveryActivityType::Undo;
        let other_job = job(&p1, "f2", "alice").with_follow(other_follow.id().clone());
        for j in [&follow_job, &undo_job, &other_job] {
            enqueue(db, j).await;
        }

        let mut conn = db.connection().await.unwrap();
        let repo = db.profile_delivery_job_repository();
        let cancelled = repo
            .cancel_undelivered_for_follow(
                &mut conn,
                follow.id(),
                DeliveryActivityType::Follow,
                "unfollowed",
                now(),
            )
            .await
            .unwrap();
        assert_eq!(cancelled, 1);
        drop(conn);
        assert_eq!(
            job_state(db, &follow_job.id).await,
            ProfileDeliveryJobState::Cancelled
        );
        assert_eq!(
            job_state(db, &undo_job.id).await,
            ProfileDeliveryJobState::Pending
        );
        assert_eq!(
            job_state(db, &other_job.id).await,
            ProfileDeliveryJobState::Pending
        );

        let t0 = now();
        let claimed = claim(db, "w", t0, 10).await;
        assert_eq!(claimed.len(), 2, "the cancelled job is never claimed");
        assert!(claimed.iter().all(|c| c.id != follow_job.id));

        // 送信中に取り消す: 結果は履歴にだけ残り、状態は Cancelled のまま。
        let in_flight = claimed.iter().find(|c| c.id == undo_job.id).unwrap();
        let mut conn = db.connection().await.unwrap();
        assert!(repo
            .cancel(&mut conn, &undo_job.id, "superseded", now())
            .await
            .unwrap());
        assert!(!repo
            .cancel(&mut conn, &undo_job.id, "again", now())
            .await
            .unwrap());
        drop(conn);
        let recorded = record(
            db,
            in_flight,
            t0,
            &DeliveryAttemptOutcome::Delivered {
                http_status: Some(202),
            },
        )
        .await;
        assert_eq!(recorded, RecordAttemptOutcome::Cancelled);
        let mut conn = db.connection().await.unwrap();
        let stored = repo
            .find_by_id(&mut conn, &undo_job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, ProfileDeliveryJobState::Cancelled);
        assert_eq!(stored.cancel_reason.as_deref(), Some("superseded"));
        assert!(stored.delivered_at.is_none());
        assert_eq!(
            repo.list_attempts(&mut conn, &undo_job.id)
                .await
                .unwrap()
                .len(),
            1
        );

        // 配送済みの job は取り消せない。
        let delivering = claimed.iter().find(|c| c.id == other_job.id).unwrap();
        drop(conn);
        record(
            db,
            delivering,
            t0,
            &DeliveryAttemptOutcome::Delivered {
                http_status: Some(200),
            },
        )
        .await;
        let mut conn = db.connection().await.unwrap();
        assert!(!repo
            .cancel(&mut conn, &other_job.id, "late", now())
            .await
            .unwrap());
        assert_eq!(
            job_state(db, &other_job.id).await,
            ProfileDeliveryJobState::Delivered
        );
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn concurrent_workers_never_claim_the_same_job() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        const JOBS: usize = 60;
        for n in 0..JOBS {
            enqueue(db, &job(&p1, &format!("a{n}"), "bob")).await;
        }
        let t0 = now();

        let mut workers = Vec::new();
        for w in 0..8 {
            let db = db.clone();
            workers.push(tokio::spawn(async move {
                let mut mine = Vec::new();
                loop {
                    let batch = claim(&db, &format!("worker-{w}"), t0, 3).await;
                    if batch.is_empty() {
                        break;
                    }
                    mine.extend(batch.into_iter().map(|c| *c.id.as_ref()));
                }
                mine
            }));
        }
        let mut all = Vec::new();
        for worker in workers {
            all.extend(worker.await.unwrap());
        }
        let unique: HashSet<_> = all.iter().copied().collect();
        assert_eq!(all.len(), JOBS, "every job is claimed exactly once");
        assert_eq!(unique.len(), JOBS, "no job is claimed twice");
    }
}

mod url_limits {
    use super::*;

    /// 1024 バイトちょうどの URL。
    fn max_url(prefix: &str) -> String {
        let head = format!("https://remote.example/{prefix}/");
        format!("{head}{}", "a".repeat(1024 - head.len()))
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn maximum_length_urls_fit_in_every_composite_index() {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let db = &iso.database;
        let (_, p1) = seed_personal_profile(db).await;
        let (_, p2) = seed_personal_profile(db).await;
        let long_actor =
            |p: &str| ProfileGraphActor::Remote(GraphActorUrl::new(max_url(p)).unwrap());
        let long_activity = |p: &str| GraphActivityId::new(max_url(p)).unwrap();
        assert_eq!(max_url("x").len(), 1024);

        // 世代の一意索引 (follower_actor_url, activity_id) と有効な世代の索引。
        let inbound = ProfileFollow::new_pending(
            ProfileFollowId::new(kernel::generate_id()),
            long_actor("follower"),
            local(&p1),
            long_activity("follow"),
            now(),
        )
        .unwrap();
        assert_eq!(begin(db, &inbound).await, BeginFollowOutcome::Created);
        let outbound = ProfileFollow::new_pending(
            ProfileFollowId::new(kernel::generate_id()),
            local(&p1),
            long_actor("followee"),
            long_activity("follow-out"),
            now(),
        )
        .unwrap();
        assert_eq!(begin(db, &outbound).await, BeginFollowOutcome::Created);
        assert!(matches!(
            begin(db, &inbound).await,
            BeginFollowOutcome::ReplayedActivity(_)
        ));

        // block / mute の索引。
        let mut conn = db.connection().await.unwrap();
        let block = ProfileBlock::new(
            ProfileBlockId::new(kernel::generate_id()),
            p1.clone(),
            long_actor("blocked"),
            now(),
        )
        .unwrap();
        assert!(db
            .profile_block_repository()
            .insert_if_absent(&mut conn, &block)
            .await
            .unwrap());
        let mute = ProfileMute::new(
            ProfileMuteId::new(kernel::generate_id()),
            p1.clone(),
            long_actor("muted"),
            now(),
        )
        .unwrap();
        assert!(db
            .profile_mute_repository()
            .insert_if_absent(&mut conn, &mute)
            .await
            .unwrap());

        // 重複受信記録の主キー (receiver, actor_url, activity_id)。
        let received = NewInboundActivity::new(
            p2.clone(),
            GraphActorUrl::new(max_url("actor")).unwrap(),
            long_activity("received"),
            "Follow",
            now(),
        )
        .unwrap();
        let repo = db.inbound_activity_repository();
        assert!(repo.record(&mut conn, &received).await.unwrap().first_seen);
        assert!(!repo.record(&mut conn, &received).await.unwrap().first_seen);
        drop(conn);

        // 配送 job の冪等性キー (sender, activity_id, target_inbox)。
        let long_job = NewProfileDeliveryJob::new(
            p1.clone(),
            DeliveryActivityType::Follow,
            long_activity("job"),
            GraphInboxUrl::new(max_url("inbox")).unwrap(),
            "{\"type\":\"Follow\"}",
            now(),
        )
        .unwrap();
        assert!(enqueue(db, &long_job).await.is_newly_enqueued());
        assert!(!enqueue(db, &long_job).await.is_newly_enqueued());
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn urls_over_the_limit_are_rejected_by_kernel_and_schema() {
        kernel::ensure_generator_initialized();
        let too_long = format!("{}a", max_url("x"));
        assert_eq!(too_long.len(), 1025);
        assert!(GraphActorUrl::new(too_long.clone()).is_err());
        assert!(GraphActivityId::new(too_long.clone()).is_err());
        assert!(GraphInboxUrl::new(too_long.clone()).is_err());

        let iso = IsolatedDatabase::create().await;
        let (_, p1) = seed_personal_profile(&iso.database).await;
        let blocked = sqlx::query(
            "INSERT INTO profile_blocks (id, nanoid, owner_profile_id, target_actor_url, created_at) \
             VALUES ($1, 'n1', $2, $3, now())",
        )
        .bind(kernel::generate_id())
        .bind(*p1.as_ref())
        .bind(&too_long)
        .execute(&iso.pool)
        .await;
        assert!(blocked.is_err());
        let job = sqlx::query(
            "INSERT INTO profile_delivery_jobs (id, sender_profile_id, activity_type, activity_id, \
             target_inbox, payload, state, next_attempt_at, created_at, updated_at) \
             VALUES ($1, $2, 'Follow', $3, 'https://r.example/inbox', '{}', 'pending', now(), now(), now())",
        )
        .bind(kernel::generate_id())
        .bind(*p1.as_ref())
        .bind(&too_long)
        .execute(&iso.pool)
        .await;
        assert!(job.is_err());
    }
}
