//! Cases 21, 23, 25: shared quota / cross-process rate-limit races, retention boundaries with injected
//! time (72 h, 30 d), a full quarantine. Real processes and PostgreSQL; time is injected, never waited.
use super::alerts::{evaluate, firing};
use super::db::eventually;
use super::delivery::expedite;
use super::emumet::{send_prepared, Config, Emumet, Inbound};
use super::receiver::Behavior;
use super::world::*;
use std::time::{Duration, Instant};

pub async fn burst(
    w: &World,
    targets: &[(String, String)],
    per_target: usize,
) -> (usize, usize, usize, Duration) {
    let mut prepared = std::collections::HashMap::new();
    for (_, profile) in targets {
        if !prepared.contains_key(profile) {
            let mut v = vec![];
            for i in 0..6 {
                v.push(
                    w.peer
                        .prepare(
                            profile,
                            Inbound::new(
                                serde_json::to_vec(&w.peer.note(&format!("burst {i}"))).unwrap(),
                            ),
                        )
                        .await,
                );
            }
            prepared.insert(profile.clone(), v);
        }
    }
    let started = Instant::now();
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(48));
    let mut set = tokio::task::JoinSet::new();
    for i in 0..per_target {
        for (base, profile) in targets {
            let (base, p) = (base.clone(), prepared[profile][i % 6].clone());
            let gate = gate.clone();
            set.spawn(async move {
                let _permit = gate.acquire_owned().await.unwrap();
                let r = send_prepared(&base, &p).await;
                (
                    r.status().as_u16(),
                    r.headers().get("retry-after").is_some(),
                )
            });
        }
    }
    let (mut ok, mut limited, mut other) = (0, 0, 0);
    while let Some(r) = set.join_next().await {
        match r.unwrap() {
            (202, _) => ok += 1,
            (429, true) => limited += 1,
            _ => other += 1,
        }
    }
    (ok, limited, other, started.elapsed())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case21_two_processes_share_quota_rate_limit_and_reservations() {
    let w = World::setup().await;
    let mut second_config: Config = w.config.clone();
    second_config.port = None;
    second_config.worker_id = 2;
    let second = Emumet::start(&second_config).await;
    w.receiver.set_default(Behavior::Status(503));
    let a = w.linked("c21a", 0).await;
    let b = w.linked("c21b", 1).await;
    let targets = vec![
        (w.server.base.clone(), a.profile.clone()),
        (second.base.clone(), a.profile.clone()),
        (w.server.base.clone(), b.profile.clone()),
        (second.base.clone(), b.profile.clone()),
    ];
    // A debug-build server cannot be offered 200 req/s per Profile by a real-signature client, so the
    // Profile buckets are injected: 50 tokens left and a refill clock parked in the future (no refill).
    // Both processes then consume the SAME shared bucket; exactly 50 per Profile may be granted.
    for p in [&a, &b] {
        assert_eq!(
            w.peer
                .deliver_json(&w.server, &p.profile, &w.peer.note("warm up"))
                .await
                .status(),
            202
        );
        w.db.exec(&format!("UPDATE post_relay_profile_quota SET tokens=50, token_at=clock_timestamp()+interval '1 hour' WHERE profile_id={}", p.profile_id)).await;
    }
    let (ok, limited, other, elapsed) = burst(&w, &targets, 400).await;
    eprintln!(
        "T1_QUOTA_RACE accepted={ok} limited={limited} other={other} elapsed_ms={}",
        elapsed.as_millis()
    );
    assert_eq!(other, 0, "every answer is 202 or 429+Retry-After");
    assert_eq!(ok + limited, 1600);
    // Each Profile's bucket is shared by both processes: burst 200 + refill 200/s over the run.
    let _ = elapsed;
    assert_eq!(
        ok, 100,
        "exactly 50 tokens per Profile are shared by both processes"
    );
    assert_eq!(limited, 1500);
    eprintln!("T1_QUOTA_RACE limited_by_shared_bucket={limited}");
    let stored = w.db.count("post_relay_bodies").await as usize;
    assert_eq!(stored, ok + 2, "exactly the 202 answers are durable");
    for p in [&a, &b] {
        let n =
            w.db.i64(&format!(
                "SELECT count(*) FROM post_relay_bodies WHERE profile_id={}",
                p.profile_id
            ))
            .await;
        let q =
            w.db.i64(&format!(
                "SELECT body_count FROM post_relay_profile_quota WHERE profile_id={}",
                p.profile_id
            ))
            .await;
        assert_eq!(n, q, "Profile reservation equals reality");
    }
    assert_eq!(
        w.db.i64("SELECT body_count FROM post_relay_quota").await as usize,
        stored
    );
    let alerts = evaluate(&w.db).await;
    assert!(
        firing(&alerts, "reservation_mismatch").is_none()
            && firing(&alerts, "reservation_flag_inconsistent").is_none(),
        "{alerts:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case23_72_hour_and_30_day_boundaries_with_injected_time() {
    let w = World::setup().await;
    let a = w.linked("c23", 0).await;
    let db = &w.db;

    // 72 h: a retrying job reaches its cutoff and ends at the CUTOFF, not at "now".
    w.receiver.set_default(Behavior::Status(503));
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("auto cutoff"))
            .await
            .status(),
        202
    );
    eventually("first attempt", Duration::from_secs(30), || async {
        db.i64("SELECT coalesce(max(total_attempts),0)::bigint FROM post_relay_jobs")
            .await
            >= 1
    })
    .await;
    let shift = |hours: i64| {
        format!("UPDATE post_relay_bodies SET received_at = received_at - interval '{hours} hours', automatic_expires_at = automatic_expires_at - interval '{hours} hours'")
    };
    w.db.exec("ALTER TABLE post_relay_bodies DISABLE TRIGGER post_relay_immutable_body")
        .await;
    w.db.exec(&shift(71)).await;
    w.db.exec("ALTER TABLE post_relay_bodies ENABLE TRIGGER post_relay_immutable_body")
        .await;
    expedite(&w).await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(
        w.db.count("post_relay_jobs").await,
        1,
        "71 h old: still retrying"
    );
    assert_eq!(w.db.count("post_relay_dead_letters").await, 0);
    let expiry_before: String =
        sqlx::query_scalar("SELECT automatic_expires_at::text FROM post_relay_bodies")
            .fetch_one(&w.db.iso.pool)
            .await
            .unwrap();
    w.db.exec("ALTER TABLE post_relay_bodies DISABLE TRIGGER post_relay_immutable_body")
        .await;
    w.db.exec(&shift(2)).await;
    w.db.exec("ALTER TABLE post_relay_bodies ENABLE TRIGGER post_relay_immutable_body")
        .await;
    eventually(
        "73 h old job is cut off into quarantine",
        Duration::from_secs(30),
        || async {
            db.count("post_relay_dead_letters").await == 1 && db.count("post_relay_jobs").await == 0
        },
    )
    .await;
    let (reason, terminal_equals_cutoff): (String, bool) = sqlx::query_as(
        "SELECT d.reason, d.terminal_at = b.automatic_expires_at FROM post_relay_dead_letters d JOIN post_relay_bodies b USING (receipt)",
    )
    .fetch_one(&w.db.iso.pool)
    .await
    .unwrap();
    assert_eq!(reason, "expired");
    assert!(
        terminal_equals_cutoff,
        "terminal time is the original 72 h cutoff, not the late processing time ({expiry_before})"
    );
    assert_eq!(w.db.count("post_relay_audits").await, 1);
    let audit_expiry_equals: bool = sqlx::query_scalar("SELECT a.expires_at = a.terminal_at + interval '720 hours' AND a.terminal_at = d.terminal_at FROM post_relay_audits a JOIN post_relay_dead_letters d USING (receipt)").fetch_one(&w.db.iso.pool).await.unwrap();
    assert!(audit_expiry_equals);
    let receipt: String = sqlx::query_scalar("SELECT receipt FROM post_relay_dead_letters")
        .fetch_one(&w.db.iso.pool)
        .await
        .unwrap();
    let _ = receipt;

    // 30 d: quarantine and the body-free audit stay until their original 30-day expiry, then go.
    let shift_terminal = |hours: i64| {
        format!("UPDATE post_relay_dead_letters SET terminal_at = terminal_at - interval '{hours} hours', expires_at = expires_at - interval '{hours} hours'")
    };
    let shift_audit = |hours: i64| {
        format!("UPDATE post_relay_audits SET terminal_at = terminal_at - interval '{hours} hours', expires_at = expires_at - interval '{hours} hours'")
    };
    let remaining: i64 = w.db.i64("SELECT extract(epoch FROM expires_at - clock_timestamp())::bigint FROM post_relay_dead_letters").await;
    assert!(
        remaining > 29 * 24 * 3600 && remaining <= 30 * 24 * 3600,
        "{remaining}"
    );
    // 29 days 22 hours after the cutoff: everything is still there.
    w.db.exec(&shift_terminal(718)).await;
    w.db.exec(&shift_audit(718)).await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(
        (
            w.db.count("post_relay_bodies").await,
            w.db.count("post_relay_dead_letters").await,
            w.db.count("post_relay_audits").await
        ),
        (1, 1, 1)
    );
    // Past 30 days: body, quarantine and audit are purged by maintenance.
    w.db.exec(&shift_terminal(2)).await;
    w.db.exec(&shift_audit(2)).await;
    eventually("30-day expiry purges", Duration::from_secs(30), || async {
        db.count("post_relay_bodies").await == 0
            && db.count("post_relay_dead_letters").await == 0
            && db.count("post_relay_audits").await == 0
    })
    .await;
    assert!(w.db.i64("SELECT purged FROM post_relay_counters").await >= 1);
    let alerts = evaluate(&w.db).await;
    assert!(
        firing(&alerts, "purge_delay").is_none()
            && firing(&alerts, "reservation_mismatch").is_none(),
        "{alerts:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case25_full_quarantine_rejects_new_admission_but_keeps_accepted_bodies() {
    let w = World::setup().await;
    let a = w.linked("c25a", 0).await;
    let other = w.linked("c25b", 1).await;
    w.receiver.set_default(Behavior::Status(400));
    // Fill one Profile's protected quarantine to its 2000-body limit with real permanent failures.
    let targets = vec![(w.server.base.clone(), a.profile.clone())];
    let mut sent = 0usize;
    while sent < 1950 {
        let n = 190.min(1950 - sent);
        let (ok, limited, odd, _) = burst(&w, &targets, n).await;
        assert_eq!(odd, 0);
        sent += ok;
        assert_eq!(ok + limited, n);
        if limited > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    let db = &w.db;
    eventually("1950 are quarantined", Duration::from_secs(120), || async {
        db.i64(&format!("SELECT count(*) FROM post_relay_dead_letters d JOIN post_relay_bodies b USING (receipt) WHERE b.profile_id={}", a.profile_id)).await == 1950
    })
    .await;
    // Race for the last 50 slots from two processes: the reservation is never over-granted.
    let mut second_config: Config = w.config.clone();
    second_config.port = None;
    second_config.worker_id = 2;
    let second = Emumet::start(&second_config).await;
    let race = vec![
        (w.server.base.clone(), a.profile.clone()),
        (second.base.clone(), a.profile.clone()),
    ];
    let (ok, limited, odd, _) = burst(&w, &race, 100).await;
    assert_eq!(odd, 0);
    assert_eq!(ok + limited, 200);
    assert!(
        (1..=50).contains(&ok),
        "only the 50 free slots may be granted, got {ok}"
    );
    eventually("the raced bodies are quarantined too", Duration::from_secs(60), || async {
        db.i64(&format!("SELECT count(*) FROM post_relay_dead_letters d JOIN post_relay_bodies b USING (receipt) WHERE b.profile_id={}", a.profile_id)).await == 1950 + ok as i64
    })
    .await;
    let filled = 1950 + ok as i64;
    let (n, q) = (
        w.db.i64(&format!(
            "SELECT count(*) FROM post_relay_bodies WHERE profile_id={}",
            a.profile_id
        ))
        .await,
        w.db.i64(&format!(
            "SELECT body_count FROM post_relay_profile_quota WHERE profile_id={}",
            a.profile_id
        ))
        .await,
    );
    assert_eq!((n, q), (filled, filled));
    if ok < 50 {
        let (more, _, _, _) = burst(&w, &race, 20).await;
        let _ = more;
    }
    while w
        .db
        .i64(&format!(
            "SELECT count(*) FROM post_relay_bodies WHERE profile_id={}",
            a.profile_id
        ))
        .await
        < 2000
    {
        let r = w
            .peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("fill"))
            .await;
        if r.status() != 202 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    eventually("2000 are quarantined", Duration::from_secs(60), || async {
        db.count("post_relay_dead_letters").await == 2000
    })
    .await;
    let r = w
        .peer
        .deliver_json(&w.server, &a.profile, &w.peer.note("one too many"))
        .await;
    assert_eq!(
        r.status(),
        429,
        "a full protected quarantine rejects new admission for that Profile"
    );
    assert!(r.headers().get("retry-after").is_some());
    let alerts = evaluate(&w.db).await;
    assert_eq!(
        firing(&alerts, "quota_profile_body_count").map(|x| x.severity),
        Some("critical".to_string())
    );
    // Nothing is discarded automatically, however long maintenance runs.
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(
        w.db.i64(&format!(
            "SELECT count(*) FROM post_relay_bodies WHERE profile_id={}",
            a.profile_id
        ))
        .await,
        2000
    );
    assert_eq!(w.db.count("post_relay_dead_letters").await, 2000);
    // Another Profile is unaffected.
    w.receiver.set_default(Behavior::Accept);
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &other.profile, &w.peer.note("independent"))
            .await
            .status(),
        202
    );
    w.receiver.wait_accepted(1, Duration::from_secs(30)).await;
    // The quarantine expiry (not capacity pressure) is what frees space: inject 30 days + 1 h.
    w.db.exec("UPDATE post_relay_dead_letters SET terminal_at = terminal_at - interval '721 hours', expires_at = expires_at - interval '721 hours'").await;
    w.db.exec("UPDATE post_relay_audits SET terminal_at = terminal_at - interval '721 hours', expires_at = expires_at - interval '721 hours'").await;
    eventually(
        "expiry frees the Profile",
        Duration::from_secs(120),
        || async { db.count("post_relay_bodies").await == 0 },
    )
    .await;
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("space again"))
            .await
            .status(),
        202
    );
}

async fn relay_row_counts(w: &World) -> Vec<(String, i64)> {
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT table_name::text FROM information_schema.tables WHERE table_schema='public' AND table_name LIKE 'post_relay_%' ORDER BY 1")
            .fetch_all(&w.db.iso.pool)
            .await
            .unwrap();
    let mut out = vec![];
    for t in tables {
        out.push((t.clone(), w.db.count(&t).await));
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case22_success_and_cancel_leave_no_job_attempt_or_terminal_rows() {
    let w = World::setup().await;
    let a = w.linked("c22", 0).await;
    let db = &w.db;
    // One delivery first: the per-Profile quota and per-origin schedule rows are created lazily and are
    // bounded by the number of Profiles/origins, not by deliveries.
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("warm up"))
            .await
            .status(),
        202
    );
    w.receiver.wait_accepted(1, Duration::from_secs(30)).await;
    w.receipts_gone().await;
    let baseline = relay_row_counts(&w).await;
    let counters_before = db.i64("SELECT succeeded FROM post_relay_counters").await;
    for i in 0..25 {
        assert_eq!(
            w.peer
                .deliver_json(&w.server, &a.profile, &w.peer.note(&format!("ok {i}")))
                .await
                .status(),
            202
        );
    }
    w.receiver.wait_accepted(26, Duration::from_secs(60)).await;
    w.receipts_gone().await;
    eventually("success counter", Duration::from_secs(30), || async {
        db.i64("SELECT succeeded FROM post_relay_counters").await == counters_before + 25
    })
    .await;
    // Live relation rows do not grow with the number of successes; only the fixed counter moved.
    let after = relay_row_counts(&w).await;
    assert_eq!(
        baseline, after,
        "25 successes added no per-delivery row anywhere"
    );

    // Cancellation (unlink while a body is still being retried) leaves nothing either.
    w.receiver.set_default(Behavior::Status(503));
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("to be cancelled"))
            .await
            .status(),
        202
    );
    eventually("retrying", Duration::from_secs(30), || async {
        db.i64("SELECT coalesce(max(total_attempts),0)::bigint FROM post_relay_jobs")
            .await
            >= 1
    })
    .await;
    let cancelled_before = db.i64("SELECT cancelled FROM post_relay_counters").await;
    let (status, _) = w.server.unlink(&a.user.jwt, &a.profile, &a.link_id).await;
    assert!(status == 200 || status == 204, "{status}");
    eventually("cancelled", Duration::from_secs(30), || async {
        db.count("post_relay_jobs").await == 0
            && db.count("post_relay_bodies").await == 0
            && db.i64("SELECT cancelled FROM post_relay_counters").await == cancelled_before + 1
    })
    .await;
    assert_eq!(
        (
            db.count("post_relay_dead_letters").await,
            db.count("post_relay_audits").await
        ),
        (0, 0)
    );
    for (table, rows) in relay_row_counts(&w).await {
        if let Some((_, base)) = baseline.iter().find(|(t, _)| *t == table) {
            assert!(
                rows <= *base + 1,
                "{table}: {rows} rows after a cancellation (baseline {base})"
            );
        }
    }
}
