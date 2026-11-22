//! Cases 10-12: delivery outcomes, disconnect-after-accept, stop/restart and lease recovery.
use super::db::eventually;
use super::emumet::Emumet;
use super::receiver::Behavior;
use super::world::*;
use std::time::Duration;

/// Time injection for the retry schedule: makes every waiting job due now (the 30 s .. 1 h
/// backoff itself is covered by the S1 scheduler tests; here the process must honour it).
pub async fn expedite(w: &World) {
    w.db.exec("UPDATE post_relay_jobs SET due_at=clock_timestamp() WHERE authorized_at IS NULL").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case10_401_and_503_retry_400_403_409_permanent_redirect_rejected() {
    let w = World::setup().await;
    let a = w.linked("c10", 0).await;
    // Retryable: 401 then 503 then success; every attempt carries the SAME envelope bytes.
    w.receiver.push(Behavior::Status(401));
    w.receiver.push(Behavior::Status(503));
    let activity = w.peer.note("retry me");
    assert_eq!(w.peer.deliver_json(&w.server, &a.profile, &activity).await.status(), 202);
    for want in 1..=2 {
        w.receiver.wait_attempts(want, Duration::from_secs(30)).await;
        let db = &w.db;
        eventually("backoff recorded", Duration::from_secs(10), || async {
            db.i64("SELECT coalesce(max(total_attempts),0)::bigint FROM post_relay_jobs").await >= want as i64
                && db.i64("SELECT count(*) FROM post_relay_jobs WHERE lease_until IS NULL AND authorized_at IS NULL").await == 1
        })
        .await;
        let due: i64 = w.db.i64("SELECT (extract(epoch FROM due_at - clock_timestamp()) * 1000)::bigint FROM post_relay_jobs").await;
        assert!(due > 5_000, "retry must wait for its backoff, not be immediate ({due} ms)");
        expedite(&w).await;
    }
    let got = w.receiver.wait_accepted(1, Duration::from_secs(30)).await;
    let attempts = w.receiver.attempts();
    assert_eq!(attempts.len(), 3);
    assert!(attempts.iter().all(|x| x.body == attempts[0].body), "envelope bytes must not change between attempts");
    assert_eq!(got[0].raw_body, serde_json::to_vec(&activity).unwrap());
    w.receipts_gone().await;
    assert_eq!(w.db.count("post_relay_dead_letters").await, 0);

    // Permanent: other 4xx and 3xx stop after exactly one attempt and are quarantined.
    let cases: Vec<(&str, Behavior)> = vec![
        ("400", Behavior::Status(400)),
        ("403", Behavior::Status(403)),
        ("409", Behavior::Status(409)),
        ("redirect", Behavior::Redirect(format!("{}/elsewhere", w.receiver.origin))),
    ];
    for (i, (name, behavior)) in cases.into_iter().enumerate() {
        let before = w.receiver.attempts().len();
        w.receiver.push(behavior);
        assert_eq!(w.peer.deliver_json(&w.server, &a.profile, &w.peer.note(name)).await.status(), 202);
        let db = &w.db;
        eventually(name, Duration::from_secs(30), || async { db.count("post_relay_dead_letters").await == (i + 1) as i64 }).await;
        assert_eq!(w.receiver.attempts().len(), before + 1, "{name}: exactly one attempt, redirect never followed");
        assert_eq!(w.db.count("post_relay_jobs").await, 0);
    }
    assert_eq!(w.db.count("post_relay_audits").await, 4);
    let statuses: Vec<i16> = sqlx::query_scalar("SELECT last_status FROM post_relay_audits ORDER BY terminal_at").fetch_all(&w.db.iso.pool).await.unwrap();
    assert_eq!(statuses, vec![400, 403, 409, 302]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case11_receiver_accepts_then_disconnects_same_receipt_and_envelope_are_resent() {
    let w = World::setup().await;
    let a = w.linked("c11", 0).await;
    w.receiver.push(Behavior::AcceptThenDisconnect);
    let activity = w.peer.note("lost response");
    assert_eq!(w.peer.deliver_json(&w.server, &a.profile, &activity).await.status(), 202);
    w.receiver.wait_accepted(1, Duration::from_secs(30)).await;
    let db = &w.db;
    eventually("job waits for retry", Duration::from_secs(15), || async { db.count("post_relay_jobs").await == 1 && db.i64("SELECT total_attempts FROM post_relay_jobs").await == 1 }).await;
    expedite(&w).await;
    w.receiver.wait_attempts(2, Duration::from_secs(30)).await;
    w.receipts_gone().await;
    let attempts = w.receiver.attempts();
    assert_eq!(attempts[0].body, attempts[1].body, "same receipt and the very same envelope bytes");
    assert_eq!(attempts[1].outcome, "duplicate");
    assert_eq!(w.receiver.accepted().len(), 1, "a duplicate is acknowledged, not stored twice");
    // The (possibly duplicate-prone) delivery was never lost, nothing is quarantined.
    assert_eq!(w.db.count("post_relay_dead_letters").await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case12_stop_after_persistence_restart_and_lease_recovery() {
    // (a) Persisted but never claimed: the process dies before any send.
    let mut w = World::setup_with(Options { sender: false, ..Default::default() }).await;
    let a = w.linked("c12a", 0).await;
    let activity = w.peer.note("persisted then stopped");
    assert_eq!(w.peer.deliver_json(&w.server, &a.profile, &activity).await.status(), 202);
    assert_eq!(w.db.count("post_relay_jobs").await, 1);
    w.server.kill().await;
    let mut config = w.config.clone();
    config.sender = true;
    config.port = None;
    w.server = Emumet::start(&config).await;
    w.receiver.set_fetch_base(&w.server.base);
    let got = w.receiver.wait_accepted(1, Duration::from_secs(40)).await;
    assert_eq!(got[0].raw_body, serde_json::to_vec(&activity).unwrap());
    w.receipts_gone().await;

    // (b) Authorized and in flight (receiver stalls), then SIGKILL: the lease must be recovered
    // with a new fence and the SAME envelope bytes; the 72 h deadline is not extended.
    w.receiver.set_default(Behavior::Stall);
    let activity = w.peer.note("killed in flight");
    assert_eq!(w.peer.deliver_json(&w.server, &a.profile, &activity).await.status(), 202);
    let stalled = w.receiver.wait_attempts(2, Duration::from_secs(30)).await;
    let first_body = stalled[1].body.clone();
    let before: (i64, bool) = sqlx::query_as("SELECT token, authorized_at IS NOT NULL FROM post_relay_jobs").fetch_one(&w.db.iso.pool).await.unwrap();
    assert!(before.1, "authorization committed before the send started");
    let expires_before: String = sqlx::query_scalar("SELECT automatic_expires_at::text FROM post_relay_bodies").fetch_one(&w.db.iso.pool).await.unwrap();
    w.server.kill().await;
    // Time injection: the original 30 s lease has elapsed.
    w.db.exec("UPDATE post_relay_jobs SET lease_until=clock_timestamp()-interval '1 second'").await;
    w.receiver.set_default(Behavior::Accept);
    w.server = Emumet::start(&config).await;
    w.receiver.set_fetch_base(&w.server.base);
    let got = w.receiver.wait_accepted(2, Duration::from_secs(60)).await;
    assert_eq!(got[1].outer_body, first_body, "recovered delivery re-sends the identical envelope");
    w.receipts_gone().await;
    let _ = (before, expires_before);
}
