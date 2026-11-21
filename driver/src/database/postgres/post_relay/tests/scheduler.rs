use super::*;
async fn scheduled(db: &PostgresDatabase, now: OffsetDateTime, n: u16) -> Vec<RelayLease> {
    let mut t = db.get_transaction().await.unwrap();
    let leases = at(now)
        .claim_scheduled(t.connection(), n, None)
        .await
        .unwrap();
    t.commit().await.unwrap();
    leases
}
#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn s1_scheduler_is_origin_profile_fair_excludes_saturated_origin_and_favors_deadlines() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 4, true).await;
    let now = clock();
    physical(db, now).await;
    sqlx::query("UPDATE profile_links SET origin='https://healthy.example' WHERE profile_id=$1")
        .bind(ps[3].id().as_ref())
        .execute(&d.pool)
        .await
        .unwrap();
    for i in 0..20 {
        assert_eq!(
            admit(db, &ps[0], &format!("bulk-{i:02}"), now, 3).await,
            Admission::Queued
        );
    }
    for (i, p) in ps.iter().enumerate().skip(1) {
        assert_eq!(
            admit(db, p, &format!("small-{i}"), now, 3).await,
            Admission::Queued
        );
    }
    let leases = scheduled(db, now, 4).await;
    assert!(leases.iter().any(|l| l.profile_id == *ps[3].id()));
    assert!(leases.iter().any(|l| l.profile_id == *ps[1].id()));
    assert!(leases.iter().any(|l| l.profile_id == *ps[2].id()));
    for l in &leases {
        let Authorization::Permitted(_) = authorize(db, l, now).await else {
            panic!("must have slot")
        };
    }
    let more = scheduled(db, now + Duration::seconds(1), 16).await;
    assert!(more.iter().all(|l| l.profile_id == *ps[0].id()));
    assert!(more.len() <= 1, "origin already has three live slots");
    for l in &more {
        let Authorization::Permitted(_) = authorize(db, l, now + Duration::seconds(1)).await else {
            panic!("fourth origin slot")
        };
    }
    assert!(scheduled(db, now + Duration::seconds(2), 16)
        .await
        .is_empty());
    // A fresh healthy job remains claimable while the other origin is at its limit.
    assert_eq!(
        admit(db, &ps[3], "healthy-new", now + Duration::seconds(2), 3).await,
        Admission::Queued
    );
    let healthy = scheduled(db, now + Duration::seconds(2), 16).await;
    assert_eq!(healthy.len(), 1);
    assert_eq!(healthy[0].receipt, "healthy-new");
    consistent(db, now + Duration::seconds(2)).await;
}
#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn s1_scheduler_served_profile_rotates_and_earliest_expiry_wins() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 2, true).await;
    let now = clock();
    physical(db, now).await;
    for i in 0..8 {
        assert_eq!(
            admit(db, &ps[0], &format!("a-{i}"), now, 3).await,
            Admission::Queued
        );
    }
    assert_eq!(admit(db, &ps[1], "other", now, 3).await, Admission::Queued);
    let first = scheduled(db, now, 1).await;
    let second = scheduled(db, now + Duration::milliseconds(1), 1).await;
    assert_ne!(first[0].profile_id, second[0].profile_id);
}
#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn s1_scheduler_prioritizes_an_immutable_receipt_near_its_original_deadline() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 2, true).await;
    let now = clock();
    let received = now - Duration::hours(72) + Duration::seconds(30);
    physical(db, received).await;
    assert_eq!(
        admit(db, &ps[0], "urgent", received, 3).await,
        Admission::Queued
    );
    physical(db, now).await;
    assert_eq!(admit(db, &ps[1], "fresh", now, 3).await, Admission::Queued);
    let urgent = scheduled(db, now, 1).await;
    assert_eq!(urgent[0].receipt, "urgent");
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn s1_scheduler_counts_preparing_leases_before_authorization_and_across_refills() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 5, true).await;
    let now = clock();
    physical(db, now).await;
    for (i, p) in ps.iter().enumerate() {
        sqlx::query("UPDATE profile_links SET origin=$2 WHERE profile_id=$1")
            .bind(p.id().as_ref())
            .bind(format!("https://o{i}.example"))
            .execute(&d.pool)
            .await
            .unwrap();
        for j in 0..8 {
            assert_eq!(
                admit(db, p, &format!("{i}-{j}"), now, 3).await,
                Admission::Queued
            );
        }
    }
    let leases = scheduled(db, now, 16).await;
    assert_eq!(leases.len(), 16);
    for p in &ps {
        assert!(leases.iter().filter(|l| l.profile_id == *p.id()).count() <= 4);
    }
    assert!(scheduled(db, now + Duration::milliseconds(1), 16)
        .await
        .is_empty());
    assert_eq!(
        scheduled(db, now + Duration::seconds(31), 16).await.len(),
        16
    );
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn s1_scheduler_recently_served_profile_deadline_survives_last_origin_slot() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 3, true).await;
    let now = clock();
    let received = now - AUTOMATIC_TTL + Duration::seconds(1);
    physical(db, received).await;
    assert_eq!(
        admit(db, &ps[0], "urgent-last-slot", received, 3).await,
        Admission::Queued
    );
    physical(db, now).await;
    for p in ps.iter().skip(1) {
        assert_eq!(
            admit(db, p, &format!("fresh-{}", p.id().as_ref()), now, 3).await,
            Admission::Queued
        );
    }
    sqlx::query("INSERT INTO post_relay_schedule(profile_id,last_claimed_at) VALUES($1,$2)")
        .bind(ps[0].id().as_ref())
        .bind(now)
        .execute(&d.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE post_relay_slots SET origin='https://home.example',until_at=$1 WHERE id<=3",
    )
    .bind(now + SEND_TIMEOUT)
    .execute(&d.pool)
    .await
    .unwrap();
    let leases = scheduled(db, now, 16).await;
    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0].receipt, "urgent-last-slot");
    // Once the urgent job is served and the three reservations end, ordinary Profile rotation
    // still gives both other Profiles a turn rather than feeding the recently served one.
    let later = scheduled(db, now + SEND_TIMEOUT, 2).await;
    assert_eq!(later.len(), 2);
    assert!(ps
        .iter()
        .skip(1)
        .all(|p| later.iter().any(|l| l.profile_id == *p.id())));
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn s1_scheduler_stalled_claimer_times_out_rolls_back_and_recovers() {
    use kernel::interfaces::database::TransactionManager;
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let now = clock();
    physical(db, now).await;
    assert_eq!(
        admit(db, &ps[0], "stalled", now, 3).await,
        Admission::Queued
    );
    let mut holder = db.get_transaction().await.unwrap();
    let first = at(now)
        .claim_scheduled(holder.connection(), 1, None)
        .await
        .unwrap();
    assert_eq!(first.len(), 1);
    let started = std::time::Instant::now();
    let waited = tokio::time::timeout(
        std::time::Duration::from_secs(4),
        db.transaction(|c| Box::pin(async move { at(now).claim_scheduled(c, 1, None).await })),
    )
    .await;
    assert!(waited.is_ok(), "scheduler lock wait must be bounded to 2s");
    assert!(
        waited.unwrap().is_err(),
        "lock timeout must abort the waiting transaction"
    );
    assert!(started.elapsed() >= std::time::Duration::from_millis(1800));
    drop(holder); // crashed/stopped claimer: its uncommitted lease and schedule row roll back
    let recovered = db
        .transaction(|c| Box::pin(async move { at(now).claim_scheduled(c, 1, None).await }))
        .await
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].receipt, "stalled");
    assert_ne!(recovered[0].token, first[0].token);
    let mut plain = db.get_transaction().await.unwrap();
    for name in [
        "lock_timeout",
        "statement_timeout",
        "idle_in_transaction_session_timeout",
    ] {
        let value: String = sqlx::query_scalar(&format!("SHOW {name}"))
            .fetch_one(&mut **plain.connection())
            .await
            .unwrap();
        assert_eq!(
            value, "0",
            "claim settings must not leak to the pool: {name}"
        );
    }
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn s1_scheduler_transaction_budget_includes_time_before_claim_and_rolls_back() {
    use kernel::interfaces::database::TransactionManager;
    let d = IsolatedDatabase::create().await;
    let (_, ps) = fixture(&d.database, 1, true).await;
    let now = clock();
    physical(&d.database, now).await;
    assert_eq!(
        admit(&d.database, &ps[0], "budget", now, 3).await,
        Admission::Queued
    );
    let started = std::time::Instant::now();
    let result = d
        .database
        .transaction(|c| {
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                at(now).claim_scheduled(c, 1, None).await?;
                for (name, expected) in [
                    ("lock_timeout", "2s"),
                    ("statement_timeout", "3s"),
                    ("idle_in_transaction_session_timeout", "5s"),
                ] {
                    let value: String = sqlx::query_scalar(&format!("SHOW {name}"))
                        .fetch_one(&mut **c)
                        .await
                        .unwrap();
                    assert_eq!(value, expected);
                }
                // Multiple short statements / application work cannot renew the whole-tx budget.
                for _ in 0..8 {
                    sqlx::query("SELECT pg_sleep(0.5)")
                        .execute(&mut **c)
                        .await
                        .unwrap();
                }
                Ok(())
            })
        })
        .await;
    assert!(result.is_err());
    assert!(
        started.elapsed() < std::time::Duration::from_millis(5800),
        "5s starts at transaction begin, not claim"
    );
    assert_eq!(scheduled(&d.database, now, 1).await[0].receipt, "budget");
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn s1_scheduler_measures_connection_lock_and_each_jobs_ready_queue_separately() {
    use kernel::interfaces::database::{Connection, TransactionManager};
    let d = IsolatedDatabase::create().await;
    let (_, ps) = fixture(&d.database, 1, true).await;
    let now = clock();
    physical(&d.database, now).await;
    for i in 0..2 {
        assert_eq!(
            admit(&d.database, &ps[0], &format!("wait-{i}"), now, 3).await,
            Admission::Queued
        );
    }
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*d.pool.connect_options()).clone())
        .await
        .unwrap();
    let occupied = pool.acquire().await.unwrap();
    let db = PostgresDatabase::from_pool(pool.clone());
    let mut blocker = d.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE post_relay_schedule IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let task = tokio::spawn(async move {
        db.transaction(|c| {
            Box::pin(async move {
                let leases = at(now + Duration::seconds(1))
                    .claim_scheduled(c, 2, None)
                    .await?;
                Ok((leases, c.timings()))
            })
        })
        .await
        .unwrap()
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!task.is_finished(), "first wait is pool acquisition");
    drop(occupied);
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'LOCK TABLE post_relay_schedule%')")
                .fetch_one(&d.pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    blocker.rollback().await.unwrap();
    let (leases, times) = task.await.unwrap();
    assert_eq!(leases.len(), 2);
    assert!(leases.iter().all(|l| l.queue_wait_ns == 1_000_000_000));
    assert!(times.connection_wait_ns >= 180_000_000);
    assert!(times.scheduler_lock_wait_ns >= 90_000_000);
    eprintln!(
        "S1_WAIT_INJECTED connection_ms={:.3} scheduler_lock_ms={:.3} job_queue_ms={:?}",
        times.connection_wait_ns as f64 / 1e6,
        times.scheduler_lock_wait_ns as f64 / 1e6,
        leases
            .iter()
            .map(|l| l.queue_wait_ns as f64 / 1e6)
            .collect::<Vec<_>>()
    );
    pool.close().await;
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn s1_scheduler_statement_timeout_rolls_back_its_uncommitted_lease() {
    use kernel::interfaces::database::TransactionManager;
    let d = IsolatedDatabase::create().await;
    let (_, ps) = fixture(&d.database, 1, true).await;
    let now = clock();
    physical(&d.database, now).await;
    assert_eq!(
        admit(&d.database, &ps[0], "statement", now, 3).await,
        Admission::Queued
    );
    let started = std::time::Instant::now();
    let result = d
        .database
        .transaction(|c| {
            Box::pin(async move {
                at(now).claim_scheduled(c, 1, None).await?;
                sqlx::query("SELECT pg_sleep(4)")
                    .execute(&mut **c)
                    .await
                    .convert_error()?;
                Ok(())
            })
        })
        .await;
    assert!(result.is_err());
    assert!(started.elapsed() >= std::time::Duration::from_millis(2800));
    assert!(started.elapsed() < std::time::Duration::from_millis(3800));
    assert_eq!(scheduled(&d.database, now, 1).await[0].receipt, "statement");
}
