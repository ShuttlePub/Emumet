//! T1-6 alarm injections (plan 6.6): every alert fires for its injected fault, with fixed labels
//! only, and clears again. Logs and alert rows never carry bodies, secrets or arbitrary IDs.
use super::alerts::{evaluate, firing, Alert};
use super::db::eventually;
use super::delivery::expedite;
use super::receiver::Behavior;
use super::supplier::Supplier;
use super::world::*;
use std::time::Duration;

const GIB: u64 = 1024 * 1024 * 1024;

async fn probe(w: &World, profile: &str, text: &str) -> u16 {
    w.peer
        .deliver_json(&w.server, profile, &w.peer.note(text))
        .await
        .status()
        .as_u16()
}

fn assert_clean(w: &World, alerts: &[Alert], markers: &[&str]) {
    let text = format!("{alerts:?}\n{}", w.server.logs());
    for m in markers {
        assert!(!text.contains(m), "alert rows/logs disclosed {m}");
    }
}

async fn expect_firing(w: &World, name: &str, severity: &str) {
    let end = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(alert) = firing(&evaluate(&w.db).await, name) {
            assert_eq!(alert.severity, severity, "{name} severity");
            return;
        }
        assert!(std::time::Instant::now() < end, "alert {name} never fired");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn expect_clear(w: &World, name: &str) {
    let db = &w.db;
    eventually(
        &format!("{name} clears"),
        Duration::from_secs(30),
        || async { firing(&evaluate(db).await, name).is_none() },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn alarms_missing_stale_low_capacity_quota_saturation_and_reservation_mismatch() {
    let mut w = World::setup().await;
    let a = w.linked("al1", 0).await;
    assert_eq!(probe(&w, &a.profile, "baseline").await, 202);
    let baseline = evaluate(&w.db).await;
    assert!(
        baseline.is_empty(),
        "a healthy system fires nothing: {baseline:?}"
    );
    w.supplier.stop().await;
    let url = w.db.url();

    // Missing / stale / low physical capacity.
    w.db.exec("UPDATE post_relay_capacity SET sampled_at = NULL")
        .await;
    expect_firing(&w, "physical_capacity_missing_or_stale", "critical").await;
    assert_eq!(probe(&w, &a.profile, "missing").await, 503);
    Supplier::report(&url, 50 * GIB, 100 * GIB).await;
    expect_clear(&w, "physical_capacity_missing_or_stale").await;
    w.db.exec(
        "UPDATE post_relay_capacity SET sampled_at = clock_timestamp() - interval '100 seconds'",
    )
    .await;
    expect_firing(&w, "physical_capacity_missing_or_stale", "critical").await;
    Supplier::report(&url, 9 * GIB, 100 * GIB).await;
    expect_firing(&w, "physical_capacity_threshold", "critical").await;
    Supplier::report(&url, 50 * GIB, 100 * GIB).await;
    expect_clear(&w, "physical_capacity_threshold").await;

    // Quota / quarantine saturation at 80 % warning and 95 % critical (global body count and bytes).
    let (count, bytes): (i64, i64) =
        sqlx::query_as("SELECT body_count, body_bytes FROM post_relay_quota")
            .fetch_one(&w.db.iso.pool)
            .await
            .unwrap();
    w.db.exec("UPDATE post_relay_quota SET body_count = 80000")
        .await;
    expect_firing(&w, "quota_global_body_count", "warning").await;
    w.db.exec("UPDATE post_relay_quota SET body_count = 95000")
        .await;
    expect_firing(&w, "quota_global_body_count", "critical").await;
    w.db.exec("UPDATE post_relay_quota SET body_count = 79999")
        .await;
    expect_clear(&w, "quota_global_body_count").await;
    w.db.exec("UPDATE post_relay_quota SET body_bytes = 17179869184")
        .await;
    expect_firing(&w, "quota_global_body_bytes", "warning").await;
    w.db.exec("UPDATE post_relay_quota SET body_bytes = 20401094656")
        .await;
    expect_firing(&w, "quota_global_body_bytes", "critical").await;
    // The tampered counters are also a reservation mismatch (the read-only check).
    expect_firing(&w, "reservation_mismatch", "critical").await;
    sqlx::query("UPDATE post_relay_quota SET body_count = $1, body_bytes = $2")
        .bind(count)
        .bind(bytes)
        .execute(&w.db.iso.pool)
        .await
        .unwrap();
    expect_clear(&w, "quota_global_body_bytes").await;
    expect_clear(&w, "reservation_mismatch").await;

    // Reservation mismatch on a different counter, and the stored flag.
    w.db.exec("UPDATE post_relay_quota SET delivery_rows = delivery_rows + 3")
        .await;
    expect_firing(&w, "reservation_mismatch", "critical").await;
    w.db.exec("UPDATE post_relay_quota SET delivery_rows = delivery_rows - 3, consistent = false")
        .await;
    expect_firing(&w, "reservation_flag_inconsistent", "critical").await;
    assert_eq!(
        probe(&w, &a.profile, "inconsistent").await,
        503,
        "admission stops while the flag is false"
    );
    w.db.exec("UPDATE post_relay_quota SET consistent = true")
        .await;
    expect_clear(&w, "reservation_flag_inconsistent").await;
    assert_eq!(probe(&w, &a.profile, "recovered").await, 202);
    let alerts = evaluate(&w.db).await;
    assert_clean(
        &w,
        &alerts,
        &[&w.peer.actor, "capacity probe", "ory_kratos_session"],
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn alarms_purge_delay_deadline_near_backlog_and_repeated_401() {
    let w = World::setup().await;
    let a = w.linked("al2", 0).await;
    let marker = "ALARM-BODY-MARKER-77c1";

    // Purge delay: purge of an expired quarantine is blocked, so the delay alert fires and the fixed
    // maintenance-failure line is logged; unblocking purges it and the alert clears.
    w.receiver.set_default(Behavior::Status(400));
    assert_eq!(probe(&w, &a.profile, marker).await, 202);
    let db = &w.db;
    eventually("quarantined", Duration::from_secs(30), || async {
        db.count("post_relay_dead_letters").await == 1
    })
    .await;
    w.db.exec("CREATE FUNCTION t1_block() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 't1 purge blocked'; END $$").await;
    w.db.exec("CREATE TRIGGER t1_block BEFORE DELETE ON post_relay_dead_letters FOR EACH ROW EXECUTE FUNCTION t1_block()").await;
    w.db.exec("UPDATE post_relay_dead_letters SET terminal_at = terminal_at - interval '31 days', expires_at = expires_at - interval '31 days'").await;
    w.db.exec("UPDATE post_relay_audits SET terminal_at = terminal_at - interval '31 days', expires_at = expires_at - interval '31 days'").await;
    expect_firing(&w, "purge_delay", "warning").await;
    eventually(
        "maintenance failure is logged with a fixed line",
        Duration::from_secs(20),
        || async { w.server.logs().contains("post relay maintenance failed") },
    )
    .await;
    w.db.exec("DROP TRIGGER t1_block ON post_relay_dead_letters")
        .await;
    expect_clear(&w, "purge_delay").await;
    eventually("purged", Duration::from_secs(30), || async {
        db.count("post_relay_bodies").await == 0 && db.count("post_relay_audits").await == 0
    })
    .await;
    assert!(w.db.i64("SELECT purged FROM post_relay_counters").await >= 1);

    // Deadline-near backlog: a waiting job whose 72 h cutoff is two hours away (time injection).
    w.receiver.set_default(Behavior::Status(503));
    assert_eq!(probe(&w, &a.profile, "deadline").await, 202);
    eventually(
        "first attempt recorded",
        Duration::from_secs(30),
        || async {
            db.i64("SELECT coalesce(max(total_attempts),0)::bigint FROM post_relay_jobs")
                .await
                >= 1
        },
    )
    .await;
    w.db.exec("ALTER TABLE post_relay_bodies DISABLE TRIGGER post_relay_immutable_body")
        .await;
    w.db.exec("UPDATE post_relay_bodies SET received_at = received_at - interval '70 hours', automatic_expires_at = automatic_expires_at - interval '70 hours'").await;
    w.db.exec("ALTER TABLE post_relay_bodies ENABLE TRIGGER post_relay_immutable_body")
        .await;
    expect_firing(&w, "deadline_near_backlog", "warning").await;

    // Repeated 401: the same job keeps getting 401 (a receiver that no longer trusts this Emumet).
    w.receiver.set_default(Behavior::Status(401));
    for _ in 0..3 {
        let before = db.i64("SELECT total_attempts FROM post_relay_jobs").await;
        expedite(&w).await;
        let end = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let row: (i64, Option<i16>, bool) = sqlx::query_as(
                "SELECT total_attempts, last_status, lease_until IS NOT NULL FROM post_relay_jobs",
            )
            .fetch_one(&db.iso.pool)
            .await
            .unwrap();
            if row.0 > before && !row.2 {
                assert_eq!(row.1, Some(401));
                break;
            }
            assert!(
                std::time::Instant::now() < end,
                "attempt not recorded: {row:?} (before {before})"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    expect_firing(&w, "repeated_401", "critical").await;
    let (status, _) = w.server.unlink(&a.user.jwt, &a.profile, &a.link_id).await;
    assert_eq!(status, 204);
    expect_clear(&w, "repeated_401").await;
    expect_clear(&w, "deadline_near_backlog").await;
    let alerts = evaluate(&w.db).await;
    assert_clean(&w, &alerts, &[marker, &w.peer.actor, &a.link_id]);
    assert!(
        w.server.logs().contains("post relay"),
        "the sender logs fixed lines"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn alarms_lock_wait_connection_pressure_and_projector_failure() {
    let w = World::setup().await;
    let a = w.linked("al3", 0).await;
    assert_eq!(probe(&w, &a.profile, "warm").await, 202);

    // Lock wait: an operator-style transaction holds the global quota row; admissions queue behind it.
    let mut holder = w.db.iso.pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM post_relay_quota WHERE singleton FOR UPDATE")
        .execute(&mut *holder)
        .await
        .unwrap();
    let blocked = probe(&w, &a.profile, "lock wait");
    let (status, waiting) = tokio::join!(blocked, async {
        let db = &w.db;
        let mut fired = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if firing(&evaluate(db).await, "db_lock_wait").is_some() {
                fired = true;
                break;
            }
        }
        fired
    });
    assert!(
        waiting,
        "db_lock_wait must fire while admissions wait for the quota lock"
    );
    assert!(
        status == 503 || status == 429,
        "a bounded lock wait ends in a retryable status, got {status}"
    );
    holder.rollback().await.unwrap();
    expect_clear(&w, "db_lock_wait").await;
    assert_eq!(probe(&w, &a.profile, "after lock").await, 202);

    // Connection pressure: 250 of max_connections=300 sessions.
    let hog = sqlx::postgres::PgPoolOptions::new()
        .max_connections(250)
        .min_connections(250)
        .acquire_timeout(Duration::from_secs(60))
        .connect(&w.db.url())
        .await
        .unwrap();
    expect_firing(&w, "db_connection_pressure", "warning").await;
    drop(hog);
    expect_clear(&w, "db_connection_pressure").await;

    // Projector failure: the checkpoint cannot be written, so the account projector fails every tick,
    // logs its fixed line and the lag alert fires; repairing clears it.
    // Publish the backdated checkpoint and write failure atomically. Otherwise a normal tick
    // can refresh updated_at between the UPDATE and trigger creation, hiding the lag for 60 s.
    let mut fault = w.db.iso.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE projection_checkpoints IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *fault)
        .await
        .unwrap();
    sqlx::query("UPDATE projection_checkpoints SET updated_at = clock_timestamp() - interval '2 minutes' WHERE projector_name = 'account_projector'")
        .execute(&mut *fault).await.unwrap();
    sqlx::query("CREATE FUNCTION t1_ckpt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 't1 checkpoint blocked'; END $$")
        .execute(&mut *fault).await.unwrap();
    sqlx::query("CREATE TRIGGER t1_ckpt BEFORE INSERT OR UPDATE ON projection_checkpoints FOR EACH ROW EXECUTE FUNCTION t1_ckpt()")
        .execute(&mut *fault).await.unwrap();
    fault.commit().await.unwrap();
    let _ = w
        .server
        .create_account(
            &a.user.jwt,
            &format!("t1_alp_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
        )
        .await;
    expect_firing(&w, "projector_stalled", "critical").await;
    eventually(
        "projector failure is logged",
        Duration::from_secs(20),
        || async {
            w.server
                .logs()
                .contains("account projection tailing batch failed")
        },
    )
    .await;
    w.db.exec("DROP TRIGGER t1_ckpt ON projection_checkpoints")
        .await;
    expect_clear(&w, "projector_stalled").await;
    let alerts = evaluate(&w.db).await;
    assert_clean(&w, &alerts, &[&w.peer.actor, "ory_kratos_session"]);
}
