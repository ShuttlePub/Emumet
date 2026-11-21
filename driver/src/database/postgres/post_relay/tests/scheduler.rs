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
