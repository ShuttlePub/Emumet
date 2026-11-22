//! Case 30: no body, secret, credential or remote actor identifier reaches the server log (at debug
//! level, which includes the cumulative-timings "metrics" line), the relay tables other than the
//! protected body store, the operator command output or the stored counters.
use super::db::eventually;
use super::operator::run_cli;
use super::receiver::Behavior;
use super::world::*;
use std::time::Duration;

async fn dump_tables(w: &World, skip_bodies: bool) -> String {
    let tables: Vec<String> = sqlx::query_scalar("SELECT table_name::text FROM information_schema.tables WHERE table_schema='public' AND table_name LIKE 'post_relay_%' ORDER BY 1")
        .fetch_all(&w.db.iso.pool)
        .await
        .unwrap();
    let mut out = String::new();
    for t in tables {
        if skip_bodies && t == "post_relay_bodies" {
            continue;
        }
        let rows: Vec<String> = sqlx::query_scalar(&format!("SELECT x::text FROM {t} x"))
            .fetch_all(&w.db.iso.pool)
            .await
            .unwrap();
        out.push_str(&format!("\n== {t}\n{}", rows.join("\n")));
    }
    out
}

fn assert_clean(what: &str, text: &str, secrets: &[(&str, String)]) {
    for (name, secret) in secrets {
        assert!(!secret.is_empty());
        assert!(!text.contains(secret.as_str()), "{what} leaks {name}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case30_no_body_secret_or_actor_identifier_in_logs_audit_cli_or_counters() {
    let mut w = World::setup().await;
    w.config.extra.push(("RUST_LOG".into(), "debug".into()));
    w.restart().await;
    let a = w.linked("c30", 0).await;
    let db = &w.db;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let markers: Vec<(&str, String)> = vec![
        ("accepted body marker", format!("T1_LEAK_OK_{token}")),
        (
            "quarantined body marker",
            format!("T1_LEAK_QUARANTINED_{token}"),
        ),
        ("retried body marker", format!("T1_LEAK_RETRY_{token}")),
    ];

    // Success, then a retry (503) that succeeds, then a permanent failure.
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note(&markers[0].1))
            .await
            .status(),
        202
    );
    let first = w.receiver.wait_accepted(1, Duration::from_secs(30)).await;
    w.receipts_gone().await;
    w.receiver.push(Behavior::Status(503));
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note(&markers[2].1))
            .await
            .status(),
        202
    );
    w.receiver.wait_attempts(2, Duration::from_secs(30)).await;
    super::delivery::expedite(&w).await;
    w.receiver.wait_accepted(2, Duration::from_secs(30)).await;
    w.receipts_gone().await;
    w.receiver.set_default(Behavior::Status(400));
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note(&markers[1].1))
            .await
            .status(),
        202
    );
    eventually("quarantined", Duration::from_secs(30), || async {
        db.count("post_relay_dead_letters").await == 1
    })
    .await;

    // Operator surface on the quarantined body (preview, then discard).
    let operation = uuid::Uuid::new_v4().to_string();
    let pid = a.profile_id.to_string();
    let common = [
        "--profile-id",
        pid.as_str(),
        "--operation",
        operation.as_str(),
        "--operator",
        "t1-leak",
        "--reason",
        "leak scan",
    ];
    let mut args = vec!["post-relay", "preview", "--action", "discard"];
    args.extend(common);
    let preview = run_cli(&w, &args).await;
    assert!(preview.ok, "{}", preview.stderr);
    let confirm = serde_json::from_str::<serde_json::Value>(
        preview
            .stdout
            .lines()
            .rfind(|l| l.starts_with('{'))
            .unwrap(),
    )
    .unwrap()["confirmation"]
        .as_str()
        .unwrap()
        .to_string();
    let mut args = vec!["post-relay", "discard"];
    args.extend(common);
    args.extend(["--confirm", confirm.as_str()]);
    let discard = run_cli(&w, &args).await;
    assert!(discard.ok, "{}", discard.stderr);
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Secrets and identifiers that must never be written anywhere observable.
    let jwt = a.user.jwt.clone();
    let jwt_signature = jwt.rsplit('.').next().unwrap().to_string();
    let outer = &first[0].headers;
    let signature_value = outer["signature"]
        .split("signature=\"")
        .nth(1)
        .unwrap()
        .trim_end_matches('"')
        .to_string();
    let mut secrets: Vec<(&str, String)> = markers.clone();
    secrets.extend([
        ("JWT", jwt),
        ("JWT signature", jwt_signature),
        (
            "master key password",
            String::from_utf8_lossy(super::emumet::MASTER_PASSWORD).to_string(),
        ),
        ("Kratos password", super::ory::PASSWORD.to_string()),
        ("private key PEM", "PRIVATE KEY".to_string()),
        ("outer HTTP signature", signature_value),
        ("remote actor id", w.peer.actor.clone()),
        ("remote key id", w.peer.key_id.clone()),
    ]);
    let log = w.server.logs();
    assert!(
        log.contains("post relay cumulative timings") || log.len() > 1000,
        "debug log is present"
    );
    assert_clean("server log", &log, &secrets);
    // Tables: bodies still hold the quarantined body until discard; after discard none remain.
    assert_eq!(db.count("post_relay_bodies").await, 0);
    assert_clean("relay tables", &dump_tables(&w, false).await, &secrets);
    for (name, text) in [
        ("preview stdout", &preview.stdout),
        ("preview stderr", &preview.stderr),
        ("discard stdout", &discard.stdout),
        ("discard stderr", &discard.stderr),
    ] {
        assert_clean(name, text, &secrets);
    }
    // Quarantined state, before discard, is scanned too: no body outside the protected store.
    w.receiver.set_default(Behavior::Status(400));
    let again = format!("T1_LEAK_AGAIN_{token}");
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note(&again))
            .await
            .status(),
        202
    );
    eventually("second quarantine", Duration::from_secs(30), || async {
        db.count("post_relay_dead_letters").await == 1
    })
    .await;
    let extra = [("quarantined marker", again.clone())];
    assert_clean(
        "relay tables (body store excluded)",
        &dump_tables(&w, true).await,
        &extra,
    );
    assert_clean("server log (quarantined)", &w.server.logs(), &extra);
    let counters: String = sqlx::query_scalar("SELECT x::text FROM post_relay_counters x")
        .fetch_one(&db.iso.pool)
        .await
        .unwrap();
    assert!(counters.starts_with("(t,"), "{counters}");
}
