//! Cases 24 and 28: the shipped `server post-relay` operator commands (source-built, test-mode origin
//! hook) against live workers; Profile-bulk redelivery with competing workers, the 128 cap, the audit
//! trail, and redelivery of a ~31-day-old body with a fresh outer signature. Case 29 (release image)
//! lives in release_image.rs.
use super::db::eventually;
use super::emumet::Inbound;
use super::receiver::Behavior;
use super::retention::burst;
use super::world::*;
use crate::PUBLIC;
use serde_json::Value;
use std::time::{Duration, SystemTime};

pub struct Cli {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

pub async fn run_cli(w: &World, args: &[&str]) -> Cli {
    let origins = serde_json::to_string(&w.config.test_origins).unwrap();
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_server"))
        .current_dir(&w.server.dir)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env(
            "LD_LIBRARY_PATH",
            std::env::var("LD_LIBRARY_PATH").unwrap_or_default(),
        )
        .env("DATABASE_URL", w.db.url())
        .env("PUBLIC_BASE_URL", PUBLIC)
        .env(
            "POST_RELAY_ALLOWED_ORIGINS",
            serde_json::to_string(&w.config.allowed_origins).unwrap(),
        )
        .env("EMUMET_TEST_RELAY_ORIGINS", origins)
        .env("RUST_LOG", "info")
        .args(args)
        .output()
        .await
        .unwrap();
    Cli {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into(),
        stderr: String::from_utf8_lossy(&output.stderr).into(),
    }
}

fn last_json(text: &str) -> Value {
    serde_json::from_str(
        text.lines()
            .rfind(|l| l.starts_with('{'))
            .expect("json line"),
    )
    .unwrap()
}

fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}

async fn preview(w: &World, profile_id: i64, action: &str, operation: &str) -> Value {
    let pid = profile_id.to_string();
    let o = run_cli(
        w,
        &[
            "post-relay",
            "preview",
            "--action",
            action,
            "--profile-id",
            &pid,
            "--operation",
            operation,
            "--operator",
            "t1-operator",
            "--reason",
            "t1 e2e recovery",
        ],
    )
    .await;
    assert!(o.ok, "{}", o.stderr);
    last_json(&o.stdout)
}

async fn act(w: &World, action: &str, profile_id: i64, operation: &str, confirm: &str) -> Cli {
    let pid = profile_id.to_string();
    run_cli(
        w,
        &[
            "post-relay",
            action,
            "--profile-id",
            &pid,
            "--operation",
            operation,
            "--operator",
            "t1-operator",
            "--reason",
            "t1 e2e recovery",
            "--confirm",
            confirm,
        ],
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case24_profile_bulk_redeliver_and_discard_via_cli_with_competing_workers() {
    let w = World::setup().await;
    let a = w.linked("c24", 0).await;
    let db = &w.db;
    w.receiver.set_default(Behavior::Status(400));
    let targets = vec![(w.server.base.clone(), a.profile.clone())];
    let (ok, _, odd, _) = burst(&w, &targets, 140).await;
    assert_eq!((ok, odd), (140, 0));
    eventually("140 quarantined", Duration::from_secs(60), || async {
        db.count("post_relay_dead_letters").await == 140
    })
    .await;
    let total_bytes = db
        .i64("SELECT coalesce(sum(octet_length(envelope)),0)::bigint FROM post_relay_bodies")
        .await;
    let expiries: Vec<String> =
        sqlx::query_scalar("SELECT expires_at::text FROM post_relay_dead_letters ORDER BY receipt")
            .fetch_all(&db.iso.pool)
            .await
            .unwrap();

    // Preview: counts and bytes, no body.
    let operation = op();
    let p = preview(&w, a.profile_id, "redeliver", &operation).await;
    assert_eq!(p["count"], 140);
    assert!(p["bytes"].as_i64().unwrap() > 0);
    let _ = total_bytes;
    assert!(
        !p.to_string().contains("T1_BODY"),
        "preview carries no body"
    );
    let confirm = p["confirmation"].as_str().unwrap().to_string();
    // A wrong confirmation changes nothing.
    let bad = act(&w, "redeliver", a.profile_id, &operation, "0:0:0").await;
    assert!(!bad.ok);
    assert_eq!(
        db.count("post_relay_operations").await,
        0,
        "a rejected confirmation leaves no audit and no staging"
    );
    assert_eq!(db.count("post_relay_jobs").await, 0);

    // Live worker keeps running (the server's sender) while the CLI redelivers; the receiver now accepts.
    w.receiver.set_default(Behavior::Accept);
    let attempts_before = w.receiver.attempts().len();
    let first = act(&w, "redeliver", a.profile_id, &operation, &confirm).await;
    assert!(first.ok, "{}", first.stderr);
    let batch = first
        .stdout
        .lines()
        .find(|l| l.contains("batch_selected"))
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .unwrap();
    assert_eq!(
        batch["batch_selected"], 128,
        "one invocation selects at most 128"
    );
    assert_eq!(batch["selection_complete"], false);
    // The same operation with the ORIGINAL confirmation advances the bounded cursor.
    let second = act(&w, "redeliver", a.profile_id, &operation, &confirm).await;
    assert!(second.ok, "{}", second.stderr);
    let status = last_json(&second.stdout);
    assert_eq!(status["selected_count"], 140);
    assert_eq!(status["staged"], 140);
    assert_eq!(status["succeeded"], 140, "{status}");
    assert_eq!(status["pending"], 0);
    assert_eq!(status["complete"], true);
    eventually("bodies gone", Duration::from_secs(30), || async {
        db.count("post_relay_bodies").await == 0
    })
    .await;
    assert_eq!(db.count("post_relay_dead_letters").await, 0);
    let attempts = w.receiver.attempts();
    let new: Vec<_> = attempts[attempts_before..].iter().collect();
    assert_eq!(new.iter().filter(|x| x.outcome == "accepted").count(), 140);
    assert_eq!(
        new.iter().filter(|x| x.outcome == "duplicate").count(),
        0,
        "competing workers never double-send a fenced job"
    );
    assert_eq!(new.len(), 140, "exactly one attempt per body");
    let (name, reason, selected, succeeded): (String, String, i64, i64) = sqlx::query_as(
        "SELECT operator, reason, selected_count, succeeded FROM post_relay_operations WHERE id=$1",
    )
    .bind(&operation)
    .fetch_one(&db.iso.pool)
    .await
    .unwrap();
    assert_eq!(
        (name.as_str(), reason.as_str(), selected, succeeded),
        ("t1-operator", "t1 e2e recovery", 140, 140)
    );
    let quota = db.i64("SELECT body_count FROM post_relay_quota").await;
    assert_eq!(quota, 0, "remaining capacity is released");
    let _ = expiries;

    // Profile-bulk discard: separate, confirmed, irreversible, audited, never automatic.
    w.receiver.set_default(Behavior::Status(403));
    let (ok, _, odd, _) = burst(&w, &targets, 30).await;
    assert_eq!((ok, odd), (30, 0));
    eventually("30 quarantined", Duration::from_secs(60), || async {
        db.count("post_relay_dead_letters").await == 30
    })
    .await;
    let discard_op = op();
    let p = preview(&w, a.profile_id, "discard", &discard_op).await;
    assert_eq!(p["count"], 30);
    let confirm = p["confirmation"].as_str().unwrap().to_string();
    let d = act(&w, "discard", a.profile_id, &discard_op, &confirm).await;
    assert!(d.ok, "{}", d.stderr);
    assert_eq!(db.count("post_relay_bodies").await, 0);
    assert_eq!(db.count("post_relay_dead_letters").await, 0);
    let discarded: i64 = db
        .i64(&format!(
            "SELECT discarded FROM post_relay_operations WHERE id='{discard_op}'"
        ))
        .await;
    assert_eq!(discarded, 30);
    assert_eq!(db.count("post_relay_operations").await, 2);
    // Neither the CLI output nor the server log carry a body or a secret.
    for text in [
        first.stdout.as_str(),
        first.stderr.as_str(),
        second.stdout.as_str(),
        second.stderr.as_str(),
        d.stdout.as_str(),
        d.stderr.as_str(),
    ] {
        assert!(
            !text.contains("BEGIN")
                && !text.contains("test-password")
                && !text.contains("raw_body")
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case28_redelivery_near_the_retention_limit_has_a_fresh_signature_keeps_expiry_and_bytes() {
    let w = World::setup().await;
    let a = w.linked("c28", 0).await;
    let db = &w.db;
    w.receiver.set_default(Behavior::Status(400));
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("old body"))
            .await
            .status(),
        202
    );
    eventually("quarantined", Duration::from_secs(30), || async {
        db.count("post_relay_dead_letters").await == 1
    })
    .await;
    let receipt: String = sqlx::query_scalar("SELECT receipt FROM post_relay_bodies")
        .fetch_one(&db.iso.pool)
        .await
        .unwrap();
    // Age the evidence to the retention edge: received 32 days 23 hours ago, cut off at 72 h (29 days
    // 23 hours ago), so the 30-day quarantine has 1 hour left.
    db.exec("ALTER TABLE post_relay_bodies DISABLE TRIGGER post_relay_immutable_body")
        .await;
    // The immutable envelope bytes carry the same instant: age them too (trigger disabled); the CLI
    // under test must then resend exactly these old bytes.
    let old_text: String = sqlx::query_scalar(
        "SELECT to_char((received_at - interval '32 days 23 hours') AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') FROM post_relay_bodies",
    )
    .fetch_one(&db.iso.pool)
    .await
    .unwrap();
    db.exec(&format!(
        "UPDATE post_relay_bodies SET envelope = convert_to(regexp_replace(convert_from(envelope, 'UTF8'), '\"received_at\":\"[^\"]+\"', '\"received_at\":\"{old_text}\"'), 'UTF8')"
    ))
    .await;
    db.exec("UPDATE post_relay_bodies SET received_at = received_at - interval '32 days 23 hours', automatic_expires_at = automatic_expires_at - interval '32 days 23 hours'").await;
    db.exec("ALTER TABLE post_relay_bodies ENABLE TRIGGER post_relay_immutable_body")
        .await;
    db.exec("UPDATE post_relay_dead_letters SET terminal_at = terminal_at - interval '29 days 23 hours', expires_at = expires_at - interval '29 days 23 hours'").await;
    db.exec("UPDATE post_relay_audits SET terminal_at = terminal_at - interval '29 days 23 hours', expires_at = expires_at - interval '29 days 23 hours'").await;
    let expiry: String = sqlx::query_scalar("SELECT expires_at::text FROM post_relay_dead_letters")
        .fetch_one(&db.iso.pool)
        .await
        .unwrap();
    let audit_expiry: String = sqlx::query_scalar(
        "SELECT expires_at::text FROM post_relay_audits ORDER BY expires_at LIMIT 1",
    )
    .fetch_one(&db.iso.pool)
    .await
    .unwrap();
    let remaining_secs: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM expires_at - clock_timestamp())::float8 FROM post_relay_dead_letters",
    )
    .fetch_one(&db.iso.pool)
    .await
    .unwrap();
    assert!(
        (3000.0..=3600.0).contains(&remaining_secs),
        "about one hour of quarantine is left: {remaining_secs}"
    );
    let stored: Vec<u8> = sqlx::query_scalar("SELECT envelope FROM post_relay_bodies")
        .fetch_one(&db.iso.pool)
        .await
        .unwrap();
    let stored_received_at = serde_json::from_slice::<serde_json::Value>(&stored).unwrap()
        ["received_at"]
        .as_str()
        .unwrap()
        .to_string();
    let stored_age = time::OffsetDateTime::now_utc()
        - time::OffsetDateTime::parse(
            &stored_received_at,
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
    assert!(
        stored_age > time::Duration::days(32) + time::Duration::hours(22)
            && stored_age < time::Duration::days(33),
        "the stored envelope is about 32 days 23 hours old ({stored_age})"
    );

    // A failing redelivery returns to quarantine with the ORIGINAL expiry (never reset).
    w.receiver.set_default(Behavior::Status(503));
    let first = op();
    let p = preview(&w, a.profile_id, "redeliver", &first).await;
    let c = act(
        &w,
        "redeliver",
        a.profile_id,
        &first,
        p["confirmation"].as_str().unwrap(),
    )
    .await;
    assert!(c.ok, "{}", c.stderr);
    assert_eq!(last_json(&c.stdout)["failed"], 1);
    assert_eq!(db.count("post_relay_dead_letters").await, 1);
    let after: String = sqlx::query_scalar("SELECT expires_at::text FROM post_relay_dead_letters")
        .fetch_one(&db.iso.pool)
        .await
        .unwrap();
    assert_eq!(
        after, expiry,
        "a manual redelivery never extends the 30-day quarantine"
    );
    let audit_after: String = sqlx::query_scalar(
        "SELECT expires_at::text FROM post_relay_audits ORDER BY expires_at LIMIT 1",
    )
    .fetch_one(&db.iso.pool)
    .await
    .unwrap();
    assert_eq!(
        audit_after, audit_expiry,
        "the audit expiry is not extended either"
    );
    let stored_after_failure: Vec<u8> =
        sqlx::query_scalar("SELECT envelope FROM post_relay_bodies")
            .fetch_one(&db.iso.pool)
            .await
            .unwrap();
    assert_eq!(
        stored_after_failure, stored,
        "the stored envelope is byte-identical after a failed redelivery"
    );
    assert_eq!(
        w.receiver.attempts().last().map(|x| x.body.clone()),
        Some(stored.clone()),
        "even the failed attempt sent exactly the stored bytes"
    );

    // Success: the receiver verifies a FRESH outer Date/Digest/signature over the OLD immutable envelope.
    w.receiver.set_default(Behavior::Accept);
    let second = op();
    let p = preview(&w, a.profile_id, "redeliver", &second).await;
    let c = act(
        &w,
        "redeliver",
        a.profile_id,
        &second,
        p["confirmation"].as_str().unwrap(),
    )
    .await;
    assert!(c.ok, "{}", c.stderr);
    let got = w.receiver.wait_accepted(1, Duration::from_secs(30)).await;
    assert_eq!(got[0].receipt_id, receipt);
    let date = httpdate::parse_http_date(&got[0].headers["date"]).unwrap();
    let skew = SystemTime::now().duration_since(date).unwrap_or_default();
    assert!(
        skew < Duration::from_secs(120),
        "the outer Date is fresh ({skew:?})"
    );
    let received_at = got[0].envelope["received_at"].as_str().unwrap().to_string();
    let received =
        time::OffsetDateTime::parse(&received_at, &time::format_description::well_known::Rfc3339)
            .unwrap();
    let age = time::OffsetDateTime::now_utc() - received;
    assert!(
        age > time::Duration::days(32) + time::Duration::hours(22),
        "the envelope keeps its original received_at ({received_at})"
    );
    assert_eq!(
        got[0].outer_body, stored,
        "the outer body is byte-identical to the stored envelope"
    );
    assert_eq!(got[0].envelope["received_at"], stored_received_at.as_str());
    let first_signature = w
        .receiver
        .attempts()
        .iter()
        .find(|x| x.outcome == "status 503")
        .map(|x| x.headers["signature"].clone());
    assert_ne!(
        first_signature.as_deref(),
        Some(got[0].headers["signature"].as_str()),
        "each redelivery carries a new outer signature"
    );
    assert_eq!(db.count("post_relay_bodies").await, 0);
    let _ = Inbound::new(Vec::new());
}
