//! remote actor 表示情報キャッシュ repository の実 DB テスト。テストごとに使い捨ての独立 DB を使う。

use super::profile_graph_test_support::IsolatedDatabase;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::repository::{
    DependOnProfileRemoteActorCacheRepository, ProfileRemoteActorCacheRepository,
    RemoteActorCacheStatus, RemoteActorProfile,
};
use kernel::prelude::entity::GraphActorUrl;
use time::{Duration, OffsetDateTime};

fn url(name: &str) -> GraphActorUrl {
    GraphActorUrl::new(format!("https://remote.example/users/{name}")).unwrap()
}

fn profile(name: &str) -> RemoteActorProfile {
    RemoteActorProfile::new(
        name,
        Some("Display Name"),
        Some("https://remote.example/icon.png"),
    )
    .unwrap()
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_unknown_actor_has_no_row_and_an_empty_lookup_is_empty() {
    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create().await;
    let db = &iso.database;
    let repo = db.profile_remote_actor_cache_repository();
    let mut conn = db.connection().await.unwrap();
    assert!(repo.find_many(&mut conn, &[]).await.unwrap().is_empty());
    assert!(repo
        .find_many(&mut conn, &[url("nobody")])
        .await
        .unwrap()
        .is_empty());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_claim_creates_a_pending_row_and_blocks_other_claims_until_the_lease_ends() {
    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create().await;
    let db = &iso.database;
    let repo = db.profile_remote_actor_cache_repository();
    let mut conn = db.connection().await.unwrap();
    let alice = url("alice");
    let t0 = now();

    assert!(repo
        .try_claim_refresh(&mut conn, &alice, t0, t0 + Duration::seconds(60))
        .await
        .unwrap());
    let rows = repo
        .find_many(&mut conn, std::slice::from_ref(&alice))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, RemoteActorCacheStatus::Pending);
    assert!(rows[0].profile.is_none());

    // The lease holds: neither an immediate nor a later claim inside the lease succeeds.
    for offset in [0, 30, 59] {
        assert!(
            !repo
                .try_claim_refresh(
                    &mut conn,
                    &alice,
                    t0 + Duration::seconds(offset),
                    t0 + Duration::seconds(offset + 60)
                )
                .await
                .unwrap(),
            "offset {offset}"
        );
    }
    // The failed attempts did not move the lease.
    let rows = repo
        .find_many(&mut conn, std::slice::from_ref(&alice))
        .await
        .unwrap();
    assert!(rows[0].next_refresh_at < t0 + Duration::seconds(61));
    // A crashed fetcher: once the lease has passed the actor can be claimed again.
    assert!(repo
        .try_claim_refresh(
            &mut conn,
            &alice,
            t0 + Duration::seconds(61),
            t0 + Duration::seconds(121)
        )
        .await
        .unwrap());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_claims_have_exactly_one_winner() {
    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create().await;
    let alice = url("alice");
    let t0 = now();
    let mut handles = Vec::new();
    for _ in 0..16 {
        let db = iso.database.clone();
        let alice = alice.clone();
        handles.push(tokio::spawn(async move {
            let mut conn = db.connection().await.unwrap();
            db.profile_remote_actor_cache_repository()
                .try_claim_refresh(&mut conn, &alice, t0, t0 + Duration::seconds(60))
                .await
                .unwrap()
        }));
    }
    let mut winners = 0;
    for handle in handles {
        if handle.await.unwrap() {
            winners += 1;
        }
    }
    assert_eq!(winners, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_recorded_success_is_resolved_and_not_claimable_until_it_is_due() {
    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create().await;
    let db = &iso.database;
    let repo = db.profile_remote_actor_cache_repository();
    let mut conn = db.connection().await.unwrap();
    let alice = url("alice");
    let t0 = now();
    assert!(repo
        .try_claim_refresh(&mut conn, &alice, t0, t0 + Duration::seconds(60))
        .await
        .unwrap());
    repo.record_resolved(
        &mut conn,
        &alice,
        &profile("alice"),
        t0,
        t0 + Duration::hours(24),
    )
    .await
    .unwrap();

    let rows = repo
        .find_many(&mut conn, std::slice::from_ref(&alice))
        .await
        .unwrap();
    assert_eq!(rows[0].status, RemoteActorCacheStatus::Resolved);
    assert_eq!(rows[0].profile.as_ref().unwrap(), &profile("alice"));

    assert!(!repo
        .try_claim_refresh(
            &mut conn,
            &alice,
            t0 + Duration::hours(23),
            t0 + Duration::hours(23) + Duration::seconds(60)
        )
        .await
        .unwrap());
    // Due: claimable again, and the claim keeps the resolved data (a refresh in flight still serves it).
    assert!(repo
        .try_claim_refresh(
            &mut conn,
            &alice,
            t0 + Duration::hours(25),
            t0 + Duration::hours(25) + Duration::seconds(60)
        )
        .await
        .unwrap());
    let rows = repo
        .find_many(&mut conn, std::slice::from_ref(&alice))
        .await
        .unwrap();
    assert_eq!(rows[0].status, RemoteActorCacheStatus::Resolved);
    assert_eq!(rows[0].profile.as_ref().unwrap(), &profile("alice"));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_failed_fetch_marks_failed_but_keeps_earlier_data() {
    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create().await;
    let db = &iso.database;
    let repo = db.profile_remote_actor_cache_repository();
    let mut conn = db.connection().await.unwrap();
    let (alice, bob) = (url("alice"), url("bob"));
    let t0 = now();

    // Never resolved: failed, no data, retried after the recorded time.
    repo.record_failed(&mut conn, &alice, t0 + Duration::minutes(15))
        .await
        .unwrap();
    let rows = repo
        .find_many(&mut conn, std::slice::from_ref(&alice))
        .await
        .unwrap();
    assert_eq!(rows[0].status, RemoteActorCacheStatus::Failed);
    assert!(rows[0].profile.is_none());
    assert!(!repo
        .try_claim_refresh(
            &mut conn,
            &alice,
            t0 + Duration::minutes(14),
            t0 + Duration::minutes(15)
        )
        .await
        .unwrap());
    assert!(repo
        .try_claim_refresh(
            &mut conn,
            &alice,
            t0 + Duration::minutes(16),
            t0 + Duration::minutes(17)
        )
        .await
        .unwrap());
    // A later success replaces the failed state.
    repo.record_resolved(
        &mut conn,
        &alice,
        &profile("alice"),
        t0,
        t0 + Duration::hours(24),
    )
    .await
    .unwrap();
    let rows = repo
        .find_many(&mut conn, std::slice::from_ref(&alice))
        .await
        .unwrap();
    assert_eq!(rows[0].status, RemoteActorCacheStatus::Resolved);

    // Resolved earlier: a failed refresh keeps the old data and status, only the retry time moves.
    repo.record_resolved(
        &mut conn,
        &bob,
        &profile("bob"),
        t0,
        t0 + Duration::hours(24),
    )
    .await
    .unwrap();
    repo.record_failed(&mut conn, &bob, t0 + Duration::minutes(15))
        .await
        .unwrap();
    let rows = repo
        .find_many(&mut conn, std::slice::from_ref(&bob))
        .await
        .unwrap();
    assert_eq!(rows[0].status, RemoteActorCacheStatus::Resolved);
    assert_eq!(rows[0].profile.as_ref().unwrap(), &profile("bob"));
    assert!(rows[0].next_refresh_at < t0 + Duration::minutes(16));
    assert!(rows[0].next_refresh_at > t0 + Duration::minutes(14));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn find_many_returns_only_the_requested_rows_and_stores_nullable_fields() {
    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create().await;
    let db = &iso.database;
    let repo = db.profile_remote_actor_cache_repository();
    let mut conn = db.connection().await.unwrap();
    let t0 = now();
    let bare = RemoteActorProfile::new("bare", None, None).unwrap();
    for (name, p) in [
        ("a", profile("a")),
        ("b", bare.clone()),
        ("c", profile("c")),
    ] {
        repo.record_resolved(&mut conn, &url(name), &p, t0, t0 + Duration::hours(24))
            .await
            .unwrap();
    }
    let rows = repo
        .find_many(&mut conn, &[url("b"), url("c"), url("missing")])
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    let b = rows.iter().find(|r| r.actor_url == url("b")).unwrap();
    assert_eq!(b.profile.as_ref().unwrap(), &bare);
    assert_eq!(b.profile.as_ref().unwrap().display_name(), None);
    assert_eq!(b.profile.as_ref().unwrap().icon_url(), None);
    assert!(rows.iter().all(|r| r.actor_url != url("a")));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn the_schema_rejects_inconsistent_rows() {
    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create().await;
    let insert = |status: &'static str, username: Option<&'static str>, icon: String| {
        let pool = iso.pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO profile_remote_actor_cache \
                 (actor_url, status, preferred_username, icon_url, fetched_at, next_refresh_at) \
                 VALUES ($1, $2, $3, $4, now(), now())",
            )
            .bind(format!("https://remote.example/{}", kernel::generate_id()))
            .bind(status)
            .bind(username)
            .bind(icon)
            .execute(&pool)
            .await
        }
    };
    assert!(insert("resolved", Some("alice"), String::new())
        .await
        .is_ok());
    assert!(insert("resolved", None, String::new()).await.is_err());
    assert!(insert("unknown", Some("alice"), String::new())
        .await
        .is_err());
    assert!(insert("resolved", Some("alice"), "x".repeat(1025))
        .await
        .is_err());
}
