use super::*;
use crate::database::postgres::isolated_db::IsolatedDatabase;
use kernel::interfaces::database::{
    DatabaseConnection, Transaction, TransactionalDatabaseConnection,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{
    AggregateRepository, DependOnAccountRepository, DependOnProfileRepository,
};
use kernel::prelude::entity::{Account, AccountIsBot, AccountName, AuthAccountId, Nanoid, Profile};

fn at(t: OffsetDateTime) -> PostgresPostRelayStore {
    PostgresPostRelayStore { now: Some(t) }
}
fn clock() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
}
async fn fixture(db: &PostgresDatabase, count: usize, linked: bool) -> (Account, Vec<Profile>) {
    kernel::ensure_generator_initialized();
    let mut c = db.connection().await.unwrap();
    let id = AccountId::default();
    let command = if count > 1 {
        Account::create_organization(
            id.clone(),
            AccountName::new(format!("relay{}", id.as_ref())),
            AccountIsBot::new(false),
            Nanoid::default(),
            AuthAccountId::default(),
        )
    } else {
        Account::create(
            id.clone(),
            AccountName::new(format!("relay{}", id.as_ref())),
            AccountIsBot::new(false),
            Nanoid::default(),
            AuthAccountId::default(),
        )
    };
    let e = db.account_repository().save(&mut c, command).await.unwrap();
    let mut a = None;
    Account::apply(&mut a, e).unwrap();
    let a = a.unwrap();
    db.account_read_model().create(&mut c, &a).await.unwrap();
    let mut profiles = vec![];
    for _ in 0..count {
        let e = db
            .profile_repository()
            .save(
                &mut c,
                Profile::create(
                    ProfileId::new(kernel::generate_id()),
                    id.clone(),
                    None,
                    None,
                    None,
                    None,
                    Nanoid::default(),
                ),
            )
            .await
            .unwrap();
        let mut p = None;
        Profile::apply(&mut p, e).unwrap();
        let p = p.unwrap();
        let mut t = db.get_transaction().await.unwrap();
        PostgresHomeLinkStore
            .lock_accounts(t.connection(), std::slice::from_ref(&id))
            .await
            .unwrap();
        db.profile_read_model()
            .create(t.connection(), &p)
            .await
            .unwrap();
        sqlx::query("UPDATE profiles SET username=$2 WHERE id=$1")
            .bind(p.id().as_ref())
            .bind(format!("relay_{}", p.id().as_ref()))
            .execute(&mut **t.connection())
            .await
            .unwrap();
        PostgresHomeLinkStore
            .lock_profiles(t.connection(), std::slice::from_ref(p.id()))
            .await
            .unwrap();
        if linked {
            let o = PostgresHomeLinkStore
                .owner(t.connection(), p.id(), &id)
                .await
                .unwrap();
            PostgresHomeLinkStore
                .register(
                    t.connection(),
                    &o,
                    "https://emumet.example/ap/profiles/relay",
                    "https://home.example",
                )
                .await
                .unwrap();
        }
        t.commit().await.unwrap();
        profiles.push(p);
    }
    (a, profiles)
}
async fn physical(db: &PostgresDatabase, now: OffsetDateTime) {
    let mut t = db.get_transaction().await.unwrap();
    at(now)
        .report_capacity(
            t.connection(),
            &CapacityReport {
                free_bytes: 50 * 1024 * 1024 * 1024,
                total_bytes: 100 * 1024 * 1024 * 1024,
                sampled_at: now,
            },
        )
        .await
        .unwrap();
    t.commit().await.unwrap();
}
async fn admit(
    db: &PostgresDatabase,
    p: &Profile,
    receipt: &str,
    now: OffsetDateTime,
    size: usize,
) -> Admission {
    let s = at(now);
    let mut t = db.get_transaction().await.unwrap();
    let recipient = s.prepare_admission(t.connection(), p.id()).await.unwrap();
    let result = s
        .enqueue(
            t.connection(),
            &RelayAdmission {
                profile_id: p.id().clone(),
                receipt: receipt.into(),
                received_at: now,
                recipient,
                envelope: vec![b'x'; size],
            },
        )
        .await
        .unwrap();
    if matches!(result, Admission::Queued | Admission::Unlinked) {
        t.commit().await.unwrap();
    }
    result
}
async fn claim(db: &PostgresDatabase, now: OffsetDateTime, limit: u16) -> Vec<RelayLease> {
    let mut t = db.get_transaction().await.unwrap();
    let l = at(now).claim_due(t.connection(), limit).await.unwrap();
    t.commit().await.unwrap();
    l
}
async fn authorize(db: &PostgresDatabase, l: &RelayLease, now: OffsetDateTime) -> Authorization {
    let mut t = db.get_transaction().await.unwrap();
    let a = at(now)
        .authorize_send(t.connection(), l, true)
        .await
        .unwrap();
    t.commit().await.unwrap();
    a
}
async fn record(
    db: &PostgresDatabase,
    l: &RelayLease,
    now: OffsetDateTime,
    r: AttemptResult,
) -> Recorded {
    let mut t = db.get_transaction().await.unwrap();
    let a = at(now)
        .record_attempt(t.connection(), l, r, 0)
        .await
        .unwrap();
    t.commit().await.unwrap();
    a
}
async fn number(db: &IsolatedDatabase, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(&db.pool).await.unwrap()
}
async fn consistent(db: &PostgresDatabase, now: OffsetDateTime) {
    let mut t = db.get_transaction().await.unwrap();
    assert!(at(now).check_reservations(t.connection()).await.unwrap());
    t.commit().await.unwrap();
}
async fn purge(db: &PostgresDatabase, p: &Profile, now: OffsetDateTime) -> u64 {
    let mut t = db.get_transaction().await.unwrap();
    let n = at(now)
        .purge_expired(t.connection(), p.id(), 128)
        .await
        .unwrap();
    t.commit().await.unwrap();
    n
}
async fn unlink(db: &PostgresDatabase, p: &Profile) {
    let mut t = db.get_transaction().await.unwrap();
    let g = STORE.gate(t.connection(), p.id()).await.unwrap();
    let link = g.links.iter().find(|l| l.state == "active").unwrap();
    PostgresHomeLinkStore
        .unlink(t.connection(), p.id(), &link.id)
        .await
        .unwrap();
    t.commit().await.unwrap();
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_atomic_receipt_reservation_rollback_and_old_receipt_is_not_delivery_evidence() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let now = clock();
    physical(db, now).await;
    sqlx::query("CREATE TABLE test_relay_receipts(id text PRIMARY KEY)")
        .execute(&d.pool)
        .await
        .unwrap();
    {
        let mut t = db.get_transaction().await.unwrap();
        let s = at(now);
        let recipient = s.prepare_admission(t.connection(), p.id()).await.unwrap();
        sqlx::query("INSERT INTO test_relay_receipts VALUES('receipt')")
            .execute(&mut **t.connection())
            .await
            .unwrap();
        assert_eq!(
            s.enqueue(
                t.connection(),
                &RelayAdmission {
                    profile_id: p.id().clone(),
                    receipt: "receipt".into(),
                    received_at: now,
                    recipient,
                    envelope: b"  immutable body ".to_vec()
                }
            )
            .await
            .unwrap(),
            Admission::Queued
        );
        assert!(sqlx::query("SELECT 1/0")
            .execute(&mut **t.connection())
            .await
            .is_err());
    }
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_bodies").await,
        0
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM test_relay_receipts").await,
        0
    );
    assert_eq!(
        number(&d, "SELECT body_count FROM post_relay_quota").await,
        0
    );
    sqlx::query("INSERT INTO test_relay_receipts VALUES('receipt')")
        .execute(&d.pool)
        .await
        .unwrap();
    assert_eq!(admit(db, p, "receipt", now, 33).await, Admission::Queued);
    consistent(db, now).await;
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_success_volume_leaves_only_fixed_state_not_receipt_terminal_rows() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let start = clock();
    let started = std::time::Instant::now();
    for n in 0..768 {
        let now = start + Duration::seconds(n);
        if n % 60 == 0 {
            physical(db, now).await;
        }
        assert_eq!(
            admit(db, p, &format!("success-{n}"), now, 64 * 1024).await,
            Admission::Queued
        );
        let lease = claim(db, now, 1).await.remove(0);
        let Authorization::Permitted(permit) = authorize(db, &lease, now).await else {
            panic!("no permit")
        };
        assert_eq!(permit.envelope, vec![b'x'; 64 * 1024]);
        assert_eq!(permit.authorized_at, now);
        assert_eq!(permit.send_deadline, now + SEND_TIMEOUT);
        assert_eq!(
            record(db, &lease, now, AttemptResult::Success).await,
            Recorded::Applied
        );
        if n == 255 || n == 767 {
            assert_eq!(number(&d,"SELECT (SELECT count(*) FROM post_relay_bodies)+(SELECT count(*) FROM post_relay_jobs)+(SELECT count(*) FROM post_relay_attempts)+(SELECT count(*) FROM post_relay_dead_letters)+(SELECT count(*) FROM post_relay_audits)+(SELECT count(*) FROM post_relay_link_usage)").await,0);
            assert_eq!(
                number(&d, "SELECT count(*) FROM post_relay_profile_quota").await,
                1
            );
            consistent(db, now).await;
        }
    }
    assert_eq!(
        number(&d, "SELECT succeeded FROM post_relay_counters").await,
        768
    );
    assert_eq!(
        number(&d, "SELECT body_bytes FROM post_relay_quota").await,
        0
    );
    println!("Q1 row stability: 256 and 768 successes => 0 receipt/body/job/attempt/audit/link-usage rows, 1 Profile quota; {:?}",started.elapsed());
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_attempts_bounded_and_failure_has_one_summary_and_original_bytes() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let start = clock();
    physical(db, start).await;
    assert_eq!(admit(db, p, "bounded", start, 23).await, Admission::Queued);
    for n in 0..21 {
        let now = start + Duration::hours(n);
        let l = claim(db, now, 1).await.remove(0);
        assert!(matches!(
            authorize(db, &l, now).await,
            Authorization::Permitted(_)
        ));
        let r = if n == 20 {
            AttemptResult::http(403, None)
        } else {
            AttemptResult::http(401, None)
        };
        assert_eq!(record(db, &l, now, r).await, Recorded::Applied);
        if n == 19 {
            assert_eq!(
                number(&d, "SELECT count(*) FROM post_relay_attempts").await,
                16
            );
            assert_eq!(
                number(&d, "SELECT total_attempts FROM post_relay_jobs").await,
                20
            );
        }
    }
    assert_eq!(number(&d, "SELECT count(*) FROM post_relay_jobs").await, 0);
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_attempts").await,
        0
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_dead_letters").await,
        1
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_audits").await,
        1
    );
    assert_eq!(
        number(&d, "SELECT total_attempts FROM post_relay_audits").await,
        21
    );
    assert_eq!(
        number(&d, "SELECT last_status::bigint FROM post_relay_audits").await,
        403
    );
    let bytes: Vec<u8> = sqlx::query_scalar("SELECT envelope FROM post_relay_bodies")
        .fetch_one(&d.pool)
        .await
        .unwrap();
    assert_eq!(bytes, vec![b'x'; 23]);
    consistent(db, start).await;
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_exact_72_hour_and_30_day_boundaries_do_not_extend_on_hold_or_lease() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let start = clock();
    physical(db, start).await;
    admit(db, p, "expiry", start, 5).await;
    let near = start + AUTOMATIC_TTL - Duration::microseconds(1);
    let l = claim(db, near, 1).await.remove(0);
    let Authorization::Permitted(permit) = authorize(db, &l, near).await else {
        panic!("missing near-deadline permit")
    };
    assert_eq!(permit.send_deadline, start + AUTOMATIC_TTL);
    let end = start + AUTOMATIC_TTL;
    let mut t = db.get_transaction().await.unwrap();
    assert_eq!(
        at(end)
            .expire_due(t.connection(), p.id(), 128)
            .await
            .unwrap(),
        1
    );
    t.commit().await.unwrap();
    assert_eq!(
        record(db, &l, end, AttemptResult::Success).await,
        Recorded::Stale
    );
    assert_eq!(
        number(
            &d,
            "SELECT count(*) FROM post_relay_dead_letters WHERE reason='expired'"
        )
        .await,
        1
    );
    assert_eq!(
        purge(db, p, end + QUARANTINE_TTL - Duration::microseconds(1)).await,
        0
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_bodies").await,
        1
    );
    assert_eq!(purge(db, p, end + QUARANTINE_TTL).await, 2);
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_bodies").await,
        0
    );
    consistent(db, end + QUARANTINE_TTL).await;
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_stale_tokens_cannot_mutate_result_capacity_body_or_permit() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let t = clock();
    physical(db, t).await;
    admit(db, p, "fence", t, 19).await;
    let old = claim(db, t, 1).await.remove(0);
    let Authorization::Permitted(permit) = authorize(db, &old, t).await else {
        panic!("permit")
    };
    assert!(claim(db, t + SEND_TIMEOUT, 1).await.is_empty());
    let now = t + LEASE_TTL;
    let new = claim(db, now, 1).await.remove(0);
    assert!(new.token > old.token);
    assert_eq!(permit.send_deadline, t + SEND_TIMEOUT);
    for r in [
        AttemptResult::Success,
        AttemptResult::http(503, None),
        AttemptResult::http(403, None),
    ] {
        assert_eq!(record(db, &old, now, r).await, Recorded::Stale);
    }
    let mut tx = db.get_transaction().await.unwrap();
    assert_eq!(
        at(now).renew_lease(tx.connection(), &old).await.unwrap(),
        Recorded::Stale
    );
    assert_eq!(
        at(now).hold(tx.connection(), &old).await.unwrap(),
        Recorded::Stale
    );
    tx.commit().await.unwrap();
    assert!(matches!(
        authorize(db, &old, now).await,
        Authorization::Stale
    ));
    assert_eq!(
        number(&d, "SELECT body_count FROM post_relay_quota").await,
        1
    );
    assert!(matches!(
        authorize(db, &new, now).await,
        Authorization::Permitted(_)
    ));
    assert_eq!(
        record(db, &new, now, AttemptResult::Success).await,
        Recorded::Applied
    );
    consistent(db, now).await;
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_unlink_fences_new_authorization_and_stale_results_but_keeps_prior_slot_deadline(
) {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let now = clock();
    physical(db, now).await;
    admit(db, p, "prior", now, 40).await;
    let prior = claim(db, now, 1).await.remove(0);
    let Authorization::Permitted(permit) = authorize(db, &prior, now).await else {
        panic!("permit")
    };
    admit(db, p, "not-started", now, 40).await;
    let not_started = claim(db, now, 1).await.remove(0);
    unlink(db, p).await;
    assert!(claim(db, now + LEASE_TTL, 128).await.is_empty());
    assert_eq!(
        number(&d, "SELECT body_count FROM post_relay_quota").await,
        0
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_bodies").await,
        2
    );
    assert!(matches!(
        authorize(db, &not_started, now).await,
        Authorization::Stale
    ));
    assert_eq!(
        record(db, &prior, now, AttemptResult::Success).await,
        Recorded::Stale
    );
    let slot: OffsetDateTime =
        sqlx::query_scalar("SELECT until_at FROM post_relay_slots WHERE token=$1")
            .bind(prior.token)
            .fetch_one(&d.pool)
            .await
            .unwrap();
    assert_eq!(slot, permit.send_deadline);
    assert_eq!(purge(db, p, now).await, 2);
    assert_eq!(number(&d, "SELECT count(*) FROM post_relay_jobs").await, 0);
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_attempts").await,
        0
    );
    assert_eq!(
        number(&d, "SELECT cancelled FROM post_relay_counters").await,
        2
    );
    consistent(db, now).await;
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_shared_16_and_origin_4_slots_include_manual_and_survive_purge() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 21, true).await;
    let now = clock();
    physical(db, now).await;
    for (n, p) in ps.iter().enumerate() {
        // Four Profiles per origin; use immutable distinct links, not retargeted body snapshots.
        sqlx::query("UPDATE profile_links SET origin=$2 WHERE profile_id=$1")
            .bind(p.id().as_ref())
            .bind(format!(
                "https://home{}.example",
                if n == 20 { 0 } else { n / 4 }
            ))
            .execute(&d.pool)
            .await
            .unwrap();
        assert_eq!(
            admit(db, p, &format!("slots-{n:02}"), now, 2).await,
            Admission::Queued
        );
    }
    let leases = claim(db, now, 21).await;
    assert_eq!(leases.len(), 21);
    for (i, l) in leases.iter().take(16).enumerate() {
        if i == 4 {
            assert!(matches!(
                authorize(db, &leases[20], now).await,
                Authorization::Busy
            ));
        }
        assert!(matches!(
            authorize(db, l, now).await,
            Authorization::Permitted(_)
        ));
    }
    for l in leases.iter().skip(16) {
        assert!(matches!(authorize(db, l, now).await, Authorization::Busy));
    }
    assert_eq!(
        number(
            &d,
            "SELECT count(*) FROM post_relay_slots WHERE token IS NOT NULL"
        )
        .await,
        16
    );
    let l = &leases[0];
    let p = ps.iter().find(|p| p.id() == &l.profile_id).unwrap();
    unlink(db, p).await;
    purge(db, p, now).await;
    assert!(matches!(
        authorize(db, &leases[16], now).await,
        Authorization::Busy
    ));
    assert!(matches!(
        authorize(db, &leases[16], now + SEND_TIMEOUT).await,
        Authorization::Permitted(_)
    ));
    consistent(db, now).await;
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_capacity_unknown_stale_thresholds_hysteresis_and_mismatch_fail_closed() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let t = clock();
    assert_eq!(admit(db, p, "unknown", t, 1).await, Admission::Unavailable);
    let gib = 1024 * 1024 * 1024;
    for (n, free, accepting) in [
        (0, 12, true),
        (1, 10, true),
        (2, 9, false),
        (3, 11, false),
        (4, 12, true),
    ] {
        let now = t + Duration::seconds(n);
        let mut tx = db.get_transaction().await.unwrap();
        at(now)
            .report_capacity(
                tx.connection(),
                &CapacityReport {
                    free_bytes: free * gib,
                    total_bytes: 100 * gib,
                    sampled_at: now,
                },
            )
            .await
            .unwrap();
        let state = at(now).read_capacity(tx.connection()).await.unwrap();
        assert_eq!(state.physical_accepting, accepting);
        tx.commit().await.unwrap();
    }
    assert_eq!(
        admit(db, p, "fresh90", t + Duration::seconds(94), 1).await,
        Admission::Queued
    );
    assert_eq!(
        admit(
            db,
            p,
            "stale",
            t + Duration::seconds(94) + Duration::microseconds(1),
            1
        )
        .await,
        Admission::Unavailable
    );
    let mut tx = db.get_transaction().await.unwrap();
    assert!(at(t)
        .report_capacity(
            tx.connection(),
            &CapacityReport {
                free_bytes: 100 * gib,
                total_bytes: 100 * gib,
                sampled_at: t + Duration::microseconds(1)
            }
        )
        .await
        .is_err());
    drop(tx);
    physical(db, t + Duration::seconds(100)).await;
    sqlx::query("UPDATE post_relay_quota SET body_count=body_count+1")
        .execute(&d.pool)
        .await
        .unwrap();
    let mut tx = db.get_transaction().await.unwrap();
    assert!(!at(t).check_reservations(tx.connection()).await.unwrap());
    tx.commit().await.unwrap();
    assert_eq!(
        admit(db, p, "mismatch", t + Duration::seconds(100), 1).await,
        Admission::Unavailable
    );
    sqlx::query("UPDATE post_relay_quota SET body_count=body_count-1")
        .execute(&d.pool)
        .await
        .unwrap();
    consistent(db, t).await;
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_unlinked_hourly_bounded_and_exhaustion_does_not_spend_linked_reservations() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, false).await;
    let p = &ps[0];
    let t = clock();
    physical(db, t).await;
    for n in 0..100 {
        assert_eq!(
            admit(db, p, &format!("unlinked-{n}"), t, 7).await,
            Admission::Unlinked
        );
    }
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_unlinked").await,
        1
    );
    assert_eq!(
        number(&d, "SELECT count FROM post_relay_unlinked").await,
        100
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_bodies").await,
        0
    );
    // Saturate the isolated aggregate partition with bounded hour buckets and Profile rows.
    sqlx::query("INSERT INTO post_relay_profile_quota(profile_id,owner_account_id,delivery_rows,delivery_bytes,unlinked_rows) SELECT -n,1,0,0,CASE WHEN n=139 THEN 500 ELSE 721 END FROM generate_series(1,139) n").execute(&d.pool).await.unwrap();
    sqlx::query("INSERT INTO post_relay_unlinked(profile_id,reason,bucket,count,first_at,last_at,expires_at) SELECT -n,'no_home',$1::timestamptz-h*interval '1 hour',1,$1,$1,$1+interval '30 days' FROM generate_series(1,139) n CROSS JOIN generate_series(1,720) h WHERE n<139 OR h<=499").bind(t).execute(&d.pool).await.unwrap();
    sqlx::query("UPDATE post_relay_quota SET unlinked_rows=100000")
        .execute(&d.pool)
        .await
        .unwrap();
    let (_, newly_unlinked) = fixture(db, 1, false).await;
    assert_eq!(
        admit(db, &newly_unlinked[0], "header-full", t, 7).await,
        Admission::Limited {
            scope: LimitScope::Global,
            retry_after_secs: 1
        }
    );
    let (_, linked) = fixture(db, 1, true).await;
    assert_eq!(
        admit(db, &linked[0], "linked-still-open", t, 64).await,
        Admission::Queued
    );
    assert!(matches!(
        admit(db, p, "new-hour", t + Duration::hours(1), 2).await,
        Admission::Unavailable
    )); // capacity stale, never unaudited 202
    physical(db, t + Duration::hours(1)).await;
    assert!(matches!(
        admit(db, p, "new-hour", t + Duration::hours(1), 2).await,
        Admission::Limited {
            scope: LimitScope::Global,
            ..
        }
    ));
    consistent(db, t).await;
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_empty_and_populated_main_migration_operator_refuses_mismatch_without_migration()
{
    let d = IsolatedDatabase::create_without_migration(20261122000300).await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    assert!(crate::post_relay::verify_schema(&d.pool).await.is_err());
    assert_eq!(
        number(
            &d,
            "SELECT count(*) FROM _sqlx_migrations WHERE version=20261122000300"
        )
        .await,
        0
    );
    d.apply_reset().await;
    crate::post_relay::verify_schema(&d.pool).await.unwrap();
    assert_eq!(
        number(
            &d,
            "SELECT count(*) FROM profile_links WHERE state='active'"
        )
        .await,
        1
    );
    physical(db, clock()).await;
    assert_eq!(
        admit(db, &ps[0], "migrated", clock(), 7).await,
        Admission::Queued
    );
    let mut old = sqlx::migrate!("../migrations");
    old.migrations = std::borrow::Cow::Owned(
        old.iter()
            .filter(|m| m.version < 20261122000300)
            .cloned()
            .collect(),
    );
    assert!(old.run(&d.pool).await.is_err());
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_bodies").await,
        1
    );
    sqlx::query(
        "UPDATE _sqlx_migrations SET checksum=decode('00','hex') WHERE version=20261122000300",
    )
    .execute(&d.pool)
    .await
    .unwrap();
    assert!(crate::post_relay::verify_schema(&d.pool).await.is_err());
}

fn operation(p: &Profile, id: &str, action: OperatorAction) -> OperatorRequest {
    OperatorRequest {
        id: id.into(),
        profile_id: p.id().clone(),
        receipt: None,
        action,
        operator: "test-operator".into(),
        reason: "home recovered".into(),
    }
}
async fn begin(db: &PostgresDatabase, r: &OperatorRequest, now: OffsetDateTime) -> OperatorPreview {
    let mut t = db.get_transaction().await.unwrap();
    let out = at(now).begin_operator(t.connection(), r).await.unwrap();
    t.commit().await.unwrap();
    out
}
async fn operate(
    db: &PostgresDatabase,
    id: &str,
    now: OffsetDateTime,
    discard: bool,
) -> OperatorBatch {
    let mut t = db.get_transaction().await.unwrap();
    let out = if discard {
        at(now)
            .operator_discard(t.connection(), id, true)
            .await
            .unwrap()
    } else {
        at(now)
            .operator_redeliver(t.connection(), id)
            .await
            .unwrap()
    };
    t.commit().await.unwrap();
    out
}
#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_manual_one_shot_same_envelope_original_30_day_expiry_and_audit_protected() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let start = clock();
    physical(db, start).await;
    admit(db, p, "manual", start, 37).await;
    let l = claim(db, start, 1).await.remove(0);
    record(db, &l, start, AttemptResult::http(403, None)).await;
    let expires: OffsetDateTime =
        sqlx::query_scalar("SELECT expires_at FROM post_relay_dead_letters")
            .fetch_one(&d.pool)
            .await
            .unwrap();
    let now = start + Duration::days(29);
    let req = operation(p, "operation-one", OperatorAction::Redeliver);
    let selected = begin(db, &req, now).await;
    assert_eq!(selected.count, 1);
    assert_eq!(operate(db, &req.id, now, false).await.changed, 1);
    assert_eq!(operate(db, &req.id, now, false).await.changed, 0);
    let manual = claim(db, now, 1).await.remove(0);
    let Authorization::Permitted(permit) = authorize(db, &manual, now).await else {
        panic!("manual permit")
    };
    assert!(permit.manual);
    assert_eq!(permit.envelope, vec![b'x'; 37]);
    assert_eq!(permit.recipient.origin, "https://home.example");
    record(db, &manual, now, AttemptResult::http(503, None)).await;
    assert_eq!(number(&d, "SELECT count(*) FROM post_relay_jobs").await, 0);
    let after: OffsetDateTime =
        sqlx::query_scalar("SELECT expires_at FROM post_relay_dead_letters")
            .fetch_one(&d.pool)
            .await
            .unwrap();
    assert_eq!(after, expires);
    assert_eq!(
        number(&d, "SELECT failed FROM post_relay_operations").await,
        1
    );
    let req2 = operation(p, "operation-two", OperatorAction::Redeliver);
    begin(db, &req2, now).await;
    operate(db, &req2.id, now, false).await;
    let manual = claim(db, now, 1).await.remove(0);
    assert!(matches!(
        authorize(db, &manual, now).await,
        Authorization::Permitted(_)
    ));
    record(db, &manual, now, AttemptResult::Success).await;
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_bodies").await,
        0
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_audits").await,
        1
    );
    let audit_expiry: OffsetDateTime =
        sqlx::query_scalar("SELECT expires_at FROM post_relay_audits")
            .fetch_one(&d.pool)
            .await
            .unwrap();
    assert_eq!(audit_expiry, expires);
    consistent(db, now).await;
    // The failed summary remains only to its ORIGINAL deadline, not 30 days after recovery.
    purge(db, p, expires).await;
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_audits").await,
        0
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_operations").await,
        2
    );
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_operator_snapshot_batches_exclusion_discard_confirmation_and_unlink_fencing() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let start = clock();
    physical(db, start).await;
    admit(db, p, "000-pending", start, 5).await;
    for n in 0..130 {
        admit(db, p, &format!("bulk-{n:03}"), start, 5).await;
    }
    let leases = claim(db, start, 128).await;
    let pending = leases
        .iter()
        .find(|l| l.receipt == "000-pending")
        .unwrap()
        .clone();
    for l in leases.iter().filter(|l| l.receipt != "000-pending") {
        record(db, l, start, AttemptResult::http(403, None)).await;
    }
    let l = claim(db, start, 128).await;
    for l in &l {
        record(db, l, start, AttemptResult::http(403, None)).await;
    }
    let op = operation(p, "snapshot", OperatorAction::Redeliver);
    assert_eq!(begin(db, &op, start).await.count, 130);
    let op2 = operation(p, "competitor", OperatorAction::Redeliver);
    begin(db, &op2, start).await;
    // An older queued body failing AFTER selection is a new dead-letter sequence, not selected.
    record(db, &pending, start, AttemptResult::http(403, None)).await;
    assert_eq!(operate(db, &op.id, start, false).await.changed, 128);
    assert_eq!(operate(db, &op2.id, start, false).await.changed, 0);
    admit(db, p, "later-not-selected", start, 5).await;
    let later = claim(db, start, 128).await;
    for l in &later {
        record(db, l, start, AttemptResult::http(403, None)).await;
    }
    for l in &claim(db, start, 128).await {
        record(db, l, start, AttemptResult::http(403, None)).await;
    }
    let second = operate(db, &op.id, start, false).await;
    assert_eq!(second.changed, 2);
    assert!(second.complete);
    assert_eq!(
        number(
            &d,
            "SELECT count(*) FROM post_relay_jobs WHERE receipt='later-not-selected'"
        )
        .await,
        0
    );
    let discard = operation(p, "discard", OperatorAction::Discard);
    begin(db, &discard, start).await;
    let mut t = db.get_transaction().await.unwrap();
    assert!(at(start)
        .operator_discard(t.connection(), &discard.id, false)
        .await
        .is_err());
    drop(t);
    let batch = operate(db, &discard.id, start, true).await;
    assert_eq!(batch.changed, 128);
    let claimed = claim(db, start, 128).await;
    unlink(db, p).await;
    for l in &claimed {
        assert!(matches!(
            authorize(db, l, start).await,
            Authorization::Stale
        ));
    }
    let mut t = db.get_transaction().await.unwrap();
    assert!(at(start)
        .operator_redeliver(t.connection(), &op.id)
        .await
        .is_err());
    drop(t);
    while purge(db, p, start).await > 0 {}
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_bodies").await,
        0
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_audits").await,
        132
    );
    consistent(db, start).await;
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_purge_failure_rolls_back_and_reports_backlog_without_resurrecting_body() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let now = clock();
    physical(db, now).await;
    admit(db, p, "purge-failure", now, 7).await;
    unlink(db, p).await;
    sqlx::query("CREATE FUNCTION q1_fail_purge() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''injected purge failure''; END'").execute(&d.pool).await.unwrap();
    sqlx::query("CREATE TRIGGER q1_fail_purge BEFORE DELETE ON post_relay_bodies FOR EACH ROW EXECUTE FUNCTION q1_fail_purge()").execute(&d.pool).await.unwrap();
    let mut t = db.get_transaction().await.unwrap();
    assert!(at(now)
        .purge_expired(t.connection(), p.id(), 128)
        .await
        .is_err());
    drop(t);
    let mut c = db.connection().await.unwrap();
    let cap = at(now).read_capacity(&mut c).await.unwrap();
    assert_eq!(cap.cancelled_body_backlog, 1);
    assert_eq!(cap.body_count, 0);
    assert!(cap.last_purge_at.is_none());
    drop(c);
    sqlx::query("DROP TRIGGER q1_fail_purge ON post_relay_bodies")
        .execute(&d.pool)
        .await
        .unwrap();
    assert_eq!(purge(db, p, now).await, 1);
    consistent(db, now).await;
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_concurrent_admission_enforces_count_and_bytes_and_reserves_every_failure() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let start = clock();
    physical(db, start).await;
    // Count boundary. Clock injection avoids spending a rate burst as 2,000 requests in one instant.
    for n in 0..1990 {
        let now = start + Duration::milliseconds(n * 10);
        assert_eq!(
            admit(db, p, &format!("reserve-{n}"), now, 1).await,
            Admission::Queued
        );
    }
    let now = start + Duration::seconds(20);
    let mut tasks = vec![];
    for n in 0..20 {
        let db = db.clone();
        let p = p.clone();
        tasks.push(tokio::spawn(async move {
            admit(&db, &p, &format!("race-{n}"), now, 1).await
        }));
    }
    let mut accepted = 0;
    for task in tasks {
        if task.await.unwrap() == Admission::Queued {
            accepted += 1;
        }
    }
    assert_eq!(accepted, 10);
    assert_eq!(
        number(&d, "SELECT body_count FROM post_relay_quota").await,
        2000
    );
    let mut failed = 0;
    loop {
        let leases = claim(db, now, 128).await;
        if leases.is_empty() {
            break;
        }
        for l in &leases {
            record(db, l, now, AttemptResult::http(403, None)).await;
            failed += 1;
        }
    }
    assert_eq!(failed, 2000);
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_dead_letters").await,
        2000
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_audits").await,
        2000
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_attempts").await,
        0
    );
    assert!(matches!(
        admit(db, p, "no-early-eviction", now, 1).await,
        Admission::Limited {
            scope: LimitScope::Profile,
            ..
        }
    ));
    consistent(db, now).await;
    // Explicit operator audit remains available even when the body partition is entirely full.
    let discard = operation(p, "full-discard", OperatorAction::Discard);
    assert_eq!(begin(db, &discard, now).await.count, 2000);
    assert_eq!(operate(db, &discard.id, now, true).await.changed, 128);
    consistent(db, now).await;
}

#[tokio::test]
async fn process_rate_child() {
    let Ok(url) = std::env::var("Q1_RATE_CHILD_DATABASE") else {
        return;
    };
    let profile = ProfileId::new(
        std::env::var("Q1_RATE_CHILD_PROFILE")
            .unwrap()
            .parse::<i64>()
            .unwrap(),
    );
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let db = PostgresDatabase::from_pool(pool);
    let s = at(clock());
    let mut accepted = 0;
    let mut limited = 0;
    for n in 0..120 {
        let mut t = db.get_transaction().await.unwrap();
        let recipient = s.prepare_admission(t.connection(), &profile).await.unwrap();
        match s
            .enqueue(
                t.connection(),
                &RelayAdmission {
                    profile_id: profile.clone(),
                    receipt: format!("process-{}-{n}", std::process::id()),
                    received_at: clock(),
                    recipient,
                    envelope: vec![b'x'],
                },
            )
            .await
            .unwrap()
        {
            Admission::Queued => {
                t.commit().await.unwrap();
                accepted += 1;
            }
            Admission::Limited {
                scope: LimitScope::Profile,
                ..
            } => limited += 1,
            _ => panic!("unexpected child admission result"),
        }
    }
    assert_eq!(accepted + limited, 120);
    println!("shared rate child: accepted={accepted} limited={limited}");
}
#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_rate_is_shared_by_two_separate_processes_not_process_local() {
    let d = IsolatedDatabase::create().await;
    let (_, ps) = fixture(&d.database, 1, true).await;
    physical(&d.database, clock()).await;
    let mut url = url::Url::parse(&std::env::var("DATABASE_URL").unwrap()).unwrap();
    url.set_path(d.pool.connect_options().get_database().unwrap());
    let exe = std::env::current_exe().unwrap();
    let profile = ps[0].id().as_ref().to_string();
    let spawn = || {
        std::process::Command::new(&exe)
            .args([
                "--exact",
                "database::postgres::post_relay::tests::process_rate_child",
                "--nocapture",
            ])
            .env("Q1_RATE_CHILD_DATABASE", url.as_str())
            .env("Q1_RATE_CHILD_PROFILE", &profile)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    let one = spawn();
    let two = spawn();
    let (one, two) = tokio::task::spawn_blocking(move || {
        (
            one.wait_with_output().unwrap(),
            two.wait_with_output().unwrap(),
        )
    })
    .await
    .unwrap();
    assert!(
        one.status.success(),
        "{}",
        String::from_utf8_lossy(&one.stderr)
    );
    assert!(
        two.status.success(),
        "{}",
        String::from_utf8_lossy(&two.stderr)
    );
    assert_eq!(
        number(&d, "SELECT body_count FROM post_relay_quota").await,
        200
    );
    consistent(&d.database, clock()).await;
}
#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_concurrent_byte_limit_is_not_just_a_row_limit() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let start = clock();
    physical(db, start).await;
    for n in 0..250 {
        assert_eq!(
            admit(
                db,
                p,
                &format!("large-{n}"),
                start + Duration::milliseconds(n * 10),
                2 * 1024 * 1024
            )
            .await,
            Admission::Queued
        );
    }
    let now = start + Duration::seconds(3);
    let mut tasks = vec![];
    for n in 0..10 {
        let db = db.clone();
        let p = p.clone();
        tasks.push(tokio::spawn(async move {
            admit(&db, &p, &format!("large-race-{n}"), now, 2 * 1024 * 1024).await
        }));
    }
    let mut accepted = 0;
    for task in tasks {
        if task.await.unwrap() == Admission::Queued {
            accepted += 1;
        }
    }
    assert_eq!(accepted, 5);
    assert_eq!(
        number(&d, "SELECT body_count FROM post_relay_quota").await,
        255
    );
    assert_eq!(
        number(&d, "SELECT body_bytes FROM post_relay_quota").await,
        255 * (2 * 1024 * 1024 + 4096)
    );
    consistent(db, now).await;
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_p1_uncommitted_permission_bounds_revoker_wait_and_rollback_releases_slot() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let now = clock();
    physical(db, now).await;
    admit(db, p, "p1", now, 7).await;
    let l = claim(db, now, 1).await.remove(0);
    let mut permission = db.get_transaction().await.unwrap();
    assert!(matches!(
        at(now)
            .authorize_send(permission.connection(), &l, true)
            .await
            .unwrap(),
        Authorization::Permitted(_)
    ));
    let other = db.clone();
    let p2 = p.clone();
    let started = std::time::Instant::now();
    let blocked = tokio::spawn(async move {
        let mut tx = other.get_transaction().await.unwrap();
        let g = STORE.gate(tx.connection(), p2.id()).await?;
        PostgresHomeLinkStore
            .unlink(tx.connection(), p2.id(), &g.links[0].id)
            .await?;
        tx.commit().await
    });
    assert!(blocked.await.unwrap().is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    drop(permission);
    unlink(db, p).await;
    assert_eq!(
        number(
            &d,
            "SELECT count(*) FROM post_relay_slots WHERE token IS NOT NULL"
        )
        .await,
        0
    );
    assert_eq!(
        number(&d, "SELECT body_count FROM post_relay_quota").await,
        0
    );
    consistent(db, now).await;
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_full_profile_and_multi_account_logical_cancellation_is_bounded() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (a, ps) = fixture(db, 8, true).await;
    let now = clock();
    physical(db, now).await;
    // Full 512MiB logical/Profile, 2,000 bodies, half jobs with 16 attempts and half quarantine.
    // Compressible fixture keeps ordinary CI disk bounded. Initial uncompressed 4GiB TOAST
    // experiment (documented) is what rejected synchronous physical deletion in the first place.
    for p in &ps {
        let mut t = db.get_transaction().await.unwrap();
        let g = STORE.gate(t.connection(), p.id()).await.unwrap();
        STORE.quota(t.connection(), &g).await.unwrap();
        let link = g
            .links
            .iter()
            .find(|l| l.state == "active")
            .unwrap()
            .clone();
        t.commit().await.unwrap();
        // Offline fixture seeding is NOT an admission/revocation transaction. On a 2-vCPU
        // runner generating 512MiB can exceed 3s by itself. Keep that setup out of the
        // production gate budget; the measured unlink/deactivate below retain every bound.
        let mut seed = d.pool.begin().await.unwrap();
        sqlx::query("INSERT INTO post_relay_bodies(receipt,profile_id,link_id,generation,origin,received_at,automatic_expires_at,envelope,logical_bytes) SELECT 'full-'||$1::text||'-'||n,$1,$2,$3,$4,$5,$5+interval '72 hours',convert_to(repeat('x',264339+CASE WHEN n=2000 THEN 912 ELSE 0 END),'UTF8'),268435+CASE WHEN n=2000 THEN 912 ELSE 0 END FROM generate_series(1,2000) n").bind(p.id().as_ref()).bind(&link.id).bind(link.generation).bind(&link.origin).bind(now).execute(&mut *seed).await.unwrap();
        sqlx::query("INSERT INTO post_relay_jobs(receipt,due_at,total_attempts) SELECT receipt,$2,16 FROM post_relay_bodies WHERE profile_id=$1").bind(p.id().as_ref()).bind(now).execute(&mut *seed).await.unwrap();
        sqlx::query("INSERT INTO post_relay_attempts(receipt,number,at,status,reason) SELECT receipt,n,$2,503,'retry' FROM post_relay_bodies CROSS JOIN generate_series(1,16) n WHERE profile_id=$1").bind(p.id().as_ref()).bind(now).execute(&mut *seed).await.unwrap();
        sqlx::query("INSERT INTO post_relay_dead_letters(receipt,terminal_at,expires_at,reason) SELECT receipt,$2,$2+interval '30 days','permanent' FROM post_relay_bodies WHERE profile_id=$1 AND seq%2=0").bind(p.id().as_ref()).bind(now).execute(&mut *seed).await.unwrap();
        sqlx::query("INSERT INTO post_relay_audits(receipt,profile_id,link_id,reason,total_attempts,terminal_at,expires_at) SELECT b.receipt,b.profile_id,b.link_id,'permanent',16,$2,$2+interval '30 days' FROM post_relay_bodies b JOIN post_relay_dead_letters d USING(receipt) WHERE b.profile_id=$1").bind(p.id().as_ref()).bind(now).execute(&mut *seed).await.unwrap();
        sqlx::query("DELETE FROM post_relay_jobs WHERE receipt IN(SELECT receipt FROM post_relay_dead_letters)").execute(&mut *seed).await.unwrap();
        seed.commit().await.unwrap();
        let mut t = db.get_transaction().await.unwrap();
        let g = STORE.gate(t.connection(), p.id()).await.unwrap();
        STORE.quota(t.connection(), &g).await.unwrap();
        sqlx::query("INSERT INTO post_relay_link_usage(link_id,profile_id,body_count,body_bytes,auto_jobs) VALUES($1,$2,2000,536870912,1000)").bind(&link.id).bind(p.id().as_ref()).execute(&mut **t.connection()).await.unwrap();
        STORE
            .delta(
                t.connection(),
                *p.id().as_ref(),
                2000,
                536870912,
                18000,
                10_240_000,
            )
            .await
            .unwrap();
        t.commit().await.unwrap();
    }
    assert_eq!(
        number(&d, "SELECT body_count FROM post_relay_quota").await,
        16000
    );
    assert_eq!(
        number(&d, "SELECT body_bytes FROM post_relay_quota").await,
        4 * 1024 * 1024 * 1024
    );
    consistent(db, now).await;
    let start = std::time::Instant::now();
    unlink(db, &ps[0]).await;
    let single = start.elapsed();
    let mut c = db.connection().await.unwrap();
    let snapshot = db.account_repository().load(&mut c, a.id()).await.unwrap();
    let start = std::time::Instant::now();
    db.account_repository()
        .save(
            &mut c,
            Account::deactivate(a.id().clone(), snapshot.version().clone()),
        )
        .await
        .unwrap();
    let multi = start.elapsed();
    drop(c);
    assert!(single < std::time::Duration::from_secs(5));
    assert!(multi < std::time::Duration::from_secs(5));
    assert_eq!(
        number(&d, "SELECT body_count FROM post_relay_quota").await,
        0
    );
    assert_eq!(
        number(&d, "SELECT body_bytes FROM post_relay_quota").await,
        0
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_bodies").await,
        16000
    );
    consistent(db, now).await;
    let leases = claim(db, now, 128).await;
    for l in &leases {
        assert!(matches!(authorize(db, l, now).await, Authorization::Stale));
    }
    let mut worst = std::time::Duration::ZERO;
    let mut deleted = 0;
    for p in &ps {
        loop {
            let start = std::time::Instant::now();
            let n = purge(db, p, now).await;
            worst = worst.max(start.elapsed());
            assert!(n <= 128);
            deleted += n;
            if n == 0 {
                break;
            }
        }
    }
    assert_eq!(deleted, 16000);
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_attempts").await,
        0
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_audits").await,
        8000
    );
    consistent(db, now).await;
    println!("Q1 full-limit cancellation: Profile(2,000/512MiB)={single:?}, Account(7 remaining Profiles/3.5GiB)={multi:?}, physical bodies after commit=16,000; batch<=128 worst={worst:?}");
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_global_rate_and_db_received_clock_and_renew_hold_are_shared_state() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 6, true).await;
    let now = clock();
    physical(db, now).await;
    let mut tx = db.get_transaction().await.unwrap();
    assert_eq!(at(now).admission_time(tx.connection()).await.unwrap(), now);
    tx.commit().await.unwrap();
    for n in 0..1000 {
        assert_eq!(
            admit(db, &ps[n % 6], &format!("global-{n}"), now, 3).await,
            Admission::Queued
        );
    }
    assert_eq!(
        admit(db, &ps[4], "global-full", now, 3).await,
        Admission::Limited {
            scope: LimitScope::Global,
            retry_after_secs: 1
        }
    );
    let (_, unlinked) = fixture(db, 1, false).await;
    assert_eq!(
        admit(db, &unlinked[0], "independent-unlinked", now, 3).await,
        Admission::Unlinked
    );
    assert_eq!(
        admit(db, &ps[4], "global-refilled", now + Duration::seconds(1), 3).await,
        Admission::Queued
    );
    let lease = claim(db, now, 1).await.remove(0);
    let mut tx = db.get_transaction().await.unwrap();
    assert_eq!(
        at(now + Duration::seconds(10))
            .renew_lease(tx.connection(), &lease)
            .await
            .unwrap(),
        Recorded::Applied
    );
    tx.commit().await.unwrap();
    let until: OffsetDateTime =
        sqlx::query_scalar("SELECT lease_until FROM post_relay_jobs WHERE receipt=$1")
            .bind(&lease.receipt)
            .fetch_one(&d.pool)
            .await
            .unwrap();
    assert_eq!(until, now + Duration::seconds(40));
    let mut tx = db.get_transaction().await.unwrap();
    assert!(matches!(
        at(now + Duration::seconds(11))
            .authorize_send(tx.connection(), &lease, false)
            .await
            .unwrap(),
        Authorization::Held
    ));
    tx.commit().await.unwrap();
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_attempts").await,
        0
    );
    assert_eq!(
        number(
            &d,
            "SELECT count(*) FROM post_relay_slots WHERE token IS NOT NULL"
        )
        .await,
        0
    );
    consistent(db, now + Duration::seconds(11)).await;
}

#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_manual_committed_permission_is_not_repeated_after_worker_crash() {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let now = clock();
    physical(db, now).await;
    admit(db, p, "manual-crash", now, 7).await;
    let auto = claim(db, now, 1).await.remove(0);
    record(db, &auto, now, AttemptResult::http(403, None)).await;
    let req = operation(p, "manual-crash-once", OperatorAction::Redeliver);
    begin(db, &req, now).await;
    assert_eq!(operate(db, &req.id, now, false).await.changed, 1);
    let manual = claim(db, now, 1).await.remove(0);
    assert!(matches!(
        authorize(db, &manual, now).await,
        Authorization::Permitted(_)
    ));
    // Crash: committed permit but no result. Never generate a second permit in the same operation.
    let later = now + Duration::seconds(31);
    assert!(claim(db, later, 1).await.is_empty());
    let mut tx = db.get_transaction().await.unwrap();
    let due = at(later)
        .maintenance_due(tx.connection(), 128)
        .await
        .unwrap();
    assert!(due.contains(p.id()));
    assert_eq!(
        at(later)
            .expire_due(tx.connection(), p.id(), 128)
            .await
            .unwrap(),
        1
    );
    tx.commit().await.unwrap();
    assert_eq!(
        record(db, &manual, later, AttemptResult::Success).await,
        Recorded::Stale
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_bodies").await,
        1
    );
    assert_eq!(
        number(&d, "SELECT count(*) FROM post_relay_audits").await,
        1
    );
    assert_eq!(
        number(&d, "SELECT failed FROM post_relay_operations").await,
        1
    );
    let expiry: OffsetDateTime =
        sqlx::query_scalar("SELECT expires_at FROM post_relay_dead_letters")
            .fetch_one(&d.pool)
            .await
            .unwrap();
    assert_eq!(expiry, now + QUARANTINE_TTL);
    let next = operation(p, "manual-explicit-new", OperatorAction::Redeliver);
    begin(db, &next, later).await;
    assert_eq!(operate(db, &next.id, later, false).await.changed, 1);
    let l = claim(db, later, 1).await.remove(0);
    assert_ne!(l.token, manual.token);
    assert!(matches!(
        authorize(db, &l, later).await,
        Authorization::Permitted(_)
    ));
    consistent(db, later).await;
}
