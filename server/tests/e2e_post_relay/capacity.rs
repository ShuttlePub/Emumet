//! T1-4 physical capacity supply (30 s / 90 s / thresholds / hysteresis) and T1-6 alarm injections.
use super::alerts::{evaluate, firing};
use super::supplier::Supplier;
use super::world::*;
use std::time::Duration;

const GIB: u64 = 1024 * 1024 * 1024;

#[tokio::test]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn capacity_monitor_clock_diagnostic() {
    // Reproduce the former supplier's host-date -> CLI path independently of the new DB-clock
    // supplier. A monitor ahead of the DB must be refused, not silently accepted or retried.
    let db = super::db::Db::create().await;
    let (mut accepted, mut future) = (0, 0);
    for _ in 0..50 {
        let date = tokio::process::Command::new("date")
            .args(["-u", "+%Y-%m-%dT%H:%M:%S.%6NZ"])
            .output()
            .await
            .unwrap();
        assert!(date.status.success());
        let sample = String::from_utf8(date.stdout).unwrap();
        let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_server"))
            .args([
                "capacity",
                "report",
                "--free-bytes",
                "53687091200",
                "--total-bytes",
                "107374182400",
                "--sampled-at",
                sample.trim(),
            ])
            .env("DATABASE_URL", db.url())
            .output()
            .await
            .unwrap();
        if out.status.success() {
            accepted += 1;
        } else {
            let reason = String::from_utf8_lossy(&out.stderr);
            assert!(reason.contains("reason=sample_in_future"), "{reason}");
            future += 1;
        }
    }
    eprintln!("T1_MONITOR_CLOCK accepted={accepted} sample_in_future={future}");
    // Real volume measurements must publish with the DB clock even when the actual free
    // capacity is below the admission threshold. No fake healthy value or retry is used.
    for _ in 0..3 {
        Supplier::once(&db.url()).await;
    }
}

async fn probe(w: &World, profile: &str) -> (u16, Option<String>) {
    let r = w
        .peer
        .deliver_json(&w.server, profile, &w.peer.note("capacity probe"))
        .await;
    let retry = r
        .headers()
        .get("retry-after")
        .map(|v| v.to_str().unwrap().to_string());
    (r.status().as_u16(), retry)
}

async fn expect_admission(w: &World, profile: &str, accepted: bool, why: &str) {
    let (status, retry) = probe(w, profile).await;
    if accepted {
        assert_eq!(status, 202, "{why}");
    } else {
        assert_eq!(status, 503, "{why}");
        assert_eq!(retry.as_deref(), Some("1"), "{why}: Retry-After");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn capacity_supply_every_30s_fails_closed_after_90s_and_applies_thresholds_with_hysteresis() {
    let mut w = World::setup().await;
    let a = w.linked("cap", 0).await;
    let b = w.linked("capb", 1).await;
    let url = w.db.url();
    expect_admission(
        &w,
        &a.profile,
        true,
        "real df sample of the PostgreSQL volume admits",
    )
    .await;

    // The continuous supplier samples every 30 s: the stored sample advances without help.
    // Observe two consecutive advances (polling), so a slow setup on a loaded runner cannot make
    // the comparison span two periods.
    async fn sampled(w: &World) -> f64 {
        sqlx::query_scalar("SELECT extract(epoch FROM sampled_at)::float8 FROM post_relay_capacity")
            .fetch_one(&w.db.iso.pool)
            .await
            .unwrap()
    }
    let initial = sampled(&w).await;
    let mut advances = Vec::new();
    let mut last = initial;
    let deadline = std::time::Instant::now() + Duration::from_secs(150);
    while advances.len() < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "supplier did not advance twice within 150 s: {advances:?}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
        let now = sampled(&w).await;
        if now > last {
            advances.push(now);
            last = now;
        }
    }
    let period = advances[1] - advances[0];
    // Both stored sample times are cycle starts on the supplier's fixed 30 s grid, so the period
    // is 30 s itself. The +-2 s allowance covers only the jitter of the DB-clock read that follows
    // the grid tick (a docker exec on a loaded CI runner: observed 0.1-1 s) and polling at 250 ms;
    // 45 s (the old allowance, a sleep-after-work drift) is NOT a pass: 90 s freshness must hold
    // with three periods.
    assert!(
        (28.0..=32.0).contains(&period),
        "supplier period was {period}s (must be 30 s +- 2 s)"
    );
    let overruns = w.supplier.overruns();
    assert!(
        overruns.is_empty(),
        "supplier cycles overran 30 s: {overruns:?}"
    );
    let age = w.db.i64("SELECT extract(epoch FROM clock_timestamp()-sampled_at)::bigint FROM post_relay_capacity").await;
    assert!(age <= 33, "sample age {age}s");
    w.supplier.stop().await;

    // Freshness: 90 s. Time injection moves the stored sample into the past.
    w.db.exec(
        "UPDATE post_relay_capacity SET sampled_at = clock_timestamp() - interval '80 seconds'",
    )
    .await;
    expect_admission(&w, &a.profile, true, "80 s old sample is still fresh").await;
    w.db.exec(
        "UPDATE post_relay_capacity SET sampled_at = clock_timestamp() - interval '100 seconds'",
    )
    .await;
    expect_admission(&w, &a.profile, false, "100 s old sample fails closed").await;
    assert!(firing(&evaluate(&w.db).await, "physical_capacity_missing_or_stale").is_some());
    w.db.exec("UPDATE post_relay_capacity SET sampled_at = NULL")
        .await;
    expect_admission(&w, &a.profile, false, "missing sample fails closed").await;
    // A failing measurement reports nothing: freshness is allowed to expire, no invented sample.
    let warned = Supplier::failing_measurement(&url).await;
    assert!(warned.contains("measurement failed"), "{warned}");
    assert_eq!(
        w.db.i64("SELECT count(*) FROM post_relay_capacity WHERE sampled_at IS NULL")
            .await,
        1
    );
    Supplier::report(&url, 50 * GIB, 100 * GIB).await;
    expect_admission(
        &w,
        &a.profile,
        true,
        "a fresh healthy sample recovers admission",
    )
    .await;

    // Thresholds: stop below 10 GiB OR below 10 %, recover only at >= 12 GiB AND >= 12 %.
    let steps: [(u64, u64, bool, &str); 12] = [
        (
            10 * GIB,
            100 * GIB,
            true,
            "exactly 10 GiB / 10 % is allowed while healthy",
        ),
        (10 * GIB - 1, 100 * GIB, false, "just below 10 GiB stops"),
        (
            11 * GIB,
            100 * GIB,
            false,
            "11 GiB / 11 % does not recover (hysteresis)",
        ),
        (
            12 * GIB - 1,
            100 * GIB,
            false,
            "just below 12 GiB does not recover",
        ),
        (12 * GIB, 100 * GIB, true, "12 GiB / 12 % recovers"),
        (
            11 * GIB,
            100 * GIB,
            true,
            "11 GiB while healthy keeps admitting",
        ),
        (
            50 * GIB,
            1000 * GIB,
            false,
            "5 % free stops although >= 10 GiB",
        ),
        (100 * GIB, 1000 * GIB, false, "10 % does not recover"),
        (119 * GIB, 1000 * GIB, false, "11.9 % does not recover"),
        (120 * GIB, 1000 * GIB, true, "12 % and >= 12 GiB recovers"),
        (9 * GIB, 100 * GIB, false, "9 GiB stops again"),
        (500 * GIB, 1000 * GIB, true, "plenty recovers"),
    ];
    let mut cancelled_while_stopped = false;
    for (free, total, accepted, why) in steps {
        Supplier::report(&url, free, total).await;
        expect_admission(&w, &a.profile, accepted, why).await;
        let alerts = evaluate(&w.db).await;
        assert_eq!(
            firing(&alerts, "physical_capacity_threshold").is_some(),
            !accepted,
            "{why}: alert"
        );
        if !accepted && !cancelled_while_stopped {
            // Low capacity never prevents cancellation: unlinking still works.
            let (status, _) = w.server.unlink(&b.user.jwt, &b.profile, &b.link_id).await;
            assert_eq!(status, 204, "cancel while stopped");
            cancelled_while_stopped = true;
        }
    }
    // A sample that is not newer than the stored one is refused and changes nothing.
    let before: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM sampled_at)::float8 FROM post_relay_capacity",
    )
    .fetch_one(&w.db.iso.pool)
    .await
    .unwrap();
    let old = (time::OffsetDateTime::now_utc() - time::Duration::seconds(200))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_server"))
        .args([
            "capacity",
            "report",
            "--free-bytes",
            "1",
            "--total-bytes",
            "100",
            "--sampled-at",
            &old,
        ])
        .env("DATABASE_URL", &url)
        .output()
        .await
        .unwrap();
    assert!(!out.status.success(), "an old sample must be refused");
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        !text.contains("postgres://") && !text.contains("develop"),
        "no credentials in CLI errors"
    );
    let after: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM sampled_at)::float8 FROM post_relay_capacity",
    )
    .fetch_one(&w.db.iso.pool)
    .await
    .unwrap();
    assert_eq!(before, after);
}
