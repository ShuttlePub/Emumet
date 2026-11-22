//! Case 29: the operator subcommands inside the locally built RELEASE image (no test-mode, no
//! GHCR) run against the same database while a live worker keeps delivering. The container sits on the
//! compose `internal` network (database reachable, no egress) as a non-root user with a
//! read-only secret volume. Set T1_RELEASE_IMAGE to the image (e2e/post-relay/run.sh builds it).
use super::db::eventually;
use super::operator::Cli;
use super::receiver::Behavior;
use super::retention::burst;
use super::world::*;
use crate::PUBLIC;
use serde_json::Value;
use std::time::Duration;

const NETWORK: &str = "emumet-post-relay-e2e_internal";

async fn docker(args: &[String], stdin: Option<&[u8]>) -> Cli {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new("docker")
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("docker is required for case 29");
    let mut input = child.stdin.take().unwrap();
    if let Some(bytes) = stdin {
        input.write_all(bytes).await.unwrap();
    }
    drop(input);
    let o = child.wait_with_output().await.unwrap();
    Cli {
        ok: o.status.success(),
        stdout: String::from_utf8_lossy(&o.stdout).into(),
        stderr: String::from_utf8_lossy(&o.stderr).into(),
    }
}

struct Image {
    image: String,
    database: String,
    volume: String,
    test_origin: String,
}

impl Image {
    async fn run(&self, args: &[&str]) -> Cli {
        let mut a: Vec<String> = [
            "run",
            "--rm",
            "--network",
            NETWORK,
            "--user",
            "10001:10001",
            "--read-only",
            "--cap-drop",
            "ALL",
        ]
        .map(String::from)
        .to_vec();
        a.extend(["-v".into(), format!("{}:/run/secrets:ro", self.volume)]);
        for (k, v) in [
            ("DATABASE_URL", self.database.as_str()),
            ("PUBLIC_BASE_URL", PUBLIC),
            ("POST_RELAY_ALLOWED_ORIGINS", "[\"https://93.184.216.34\"]"),
            ("WORKER_ID", "not-a-worker-id"),
            ("HYDRA_ISSUER_URL", "http://127.0.0.1:9"),
            ("HTTP_LISTEN_ADDR", "not-an-address"),
            // The release binary has no test-mode: this must be ignored, never a bypass.
            ("EMUMET_TEST_RELAY_ORIGINS", self.test_origin.as_str()),
            ("EMUMET_TEST_MODE_TOKEN", "t1-e2e-test-token"),
        ] {
            a.extend(["-e".into(), format!("{k}={v}")]);
        }
        a.push(self.image.clone());
        a.extend(args.iter().map(|s| s.to_string()));
        docker(&a, None).await
    }
}

fn last_json(text: &str) -> Value {
    serde_json::from_str(
        text.lines()
            .rfind(|l| l.starts_with('{'))
            .unwrap_or_else(|| panic!("no json in {text}")),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack and T1_RELEASE_IMAGE: bash e2e/post-relay/run.sh"]
async fn case29_release_image_operator_commands_run_beside_a_live_worker() {
    let image = std::env::var("T1_RELEASE_IMAGE")
        .expect("T1_RELEASE_IMAGE must name the locally built release image");
    let w = World::setup().await;
    let a = w.linked("c29a", 0).await;
    let b = w.linked("c29b", 1).await;
    let c = w.linked("c29c", 2).await;
    let db = &w.db;

    // The secret volume: owned by the image's UID, mode 0600, mounted read-only.
    let volume = format!("emumet-t1-secret-{}", uuid::Uuid::new_v4().simple());
    let make = docker(
        &["run", "--rm", "-i", "-v", &format!("{volume}:/s"), "--user", "0", "docker.io/library/debian:bookworm-slim", "sh", "-c", "cat > /s/master-key-password && chown 10001:10001 /s/master-key-password && chmod 600 /s/master-key-password"].map(String::from),
        Some(super::emumet::MASTER_PASSWORD),
    )
    .await;
    assert!(make.ok, "{}", make.stderr);
    let mut u = url::Url::parse(&db.url()).unwrap();
    u.set_host(Some("postgres")).unwrap();
    u.set_port(Some(5432)).unwrap();
    let img = Image {
        image,
        database: u.to_string(),
        volume: volume.clone(),
        test_origin: w.receiver.origin.clone(),
    };

    // Quarantine bodies for Profile a (receiver answers 400), then let the worker serve Profile b live.
    w.receiver.set_default(Behavior::Status(400));
    let (ok, _, odd, _) = burst(&w, &[(w.server.base.clone(), a.profile.clone())], 40).await;
    assert_eq!((ok, odd), (40, 0));
    let (ok, _, odd, _) = burst(&w, &[(w.server.base.clone(), c.profile.clone())], 5).await;
    assert_eq!((ok, odd), (5, 0));
    eventually("45 quarantined", Duration::from_secs(60), || async {
        db.count("post_relay_dead_letters").await == 45
    })
    .await;
    w.receiver.set_default(Behavior::Accept);
    let attempts_before = w.receiver.attempts().len();
    let live_peer = w.peer.clone();
    let (base, profile_b) = (w.server.base.clone(), b.profile.clone());
    let live = tokio::spawn(async move {
        let mut sent = 0usize;
        for i in 0..60 {
            let r = live_peer
                .post_at(
                    &base,
                    &profile_b,
                    super::emumet::Inbound::new(
                        serde_json::to_vec(&live_peer.note(&format!("live {i}"))).unwrap(),
                    ),
                )
                .await;
            assert_eq!(r.status(), 202);
            sent += 1;
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        sent
    });

    // Image: preview (read-only), wrong confirmation, correct confirmation, audit, remaining capacity.
    let operation = uuid::Uuid::new_v4().to_string();
    let pid = a.profile_id.to_string();
    let cpid = c.profile_id.to_string();
    let common = [
        "--profile-id",
        pid.as_str(),
        "--operation",
        operation.as_str(),
        "--operator",
        "t1-image-operator",
        "--reason",
        "t1 release image",
    ];
    let mut preview_args = vec!["post-relay", "preview", "--action", "discard"];
    preview_args.extend(common);
    let p = img.run(&preview_args).await;
    assert!(p.ok, "{}", p.stderr);
    let preview = last_json(&p.stdout);
    assert_eq!(preview["count"], 40);
    let confirm = preview["confirmation"].as_str().unwrap().to_string();
    let mut wrong = vec!["post-relay", "discard"];
    wrong.extend(common);
    wrong.extend(["--confirm", "0:0:0"]);
    assert!(!img.run(&wrong).await.ok);
    assert_eq!(db.count("post_relay_operations").await, 0);
    assert_eq!(
        db.count("post_relay_dead_letters").await,
        45,
        "a wrong confirmation discards nothing"
    );
    let mut right = vec!["post-relay", "discard"];
    right.extend(common);
    right.extend(["--confirm", confirm.as_str()]);
    let d = img.run(&right).await;
    assert!(d.ok, "{}{}", d.stdout, d.stderr);
    let (operator, discarded): (String, i64) =
        sqlx::query_as("SELECT operator, discarded FROM post_relay_operations WHERE id=$1")
            .bind(&operation)
            .fetch_one(&db.iso.pool)
            .await
            .unwrap();
    assert_eq!((operator.as_str(), discarded), ("t1-image-operator", 40));
    let status_args = ["post-relay", "status", "--operation", operation.as_str()];
    let s = img.run(&status_args).await;
    assert!(s.ok, "{}", s.stderr);
    assert_eq!(last_json(&s.stdout)["discarded"], 40);

    // Image redeliver: the quarantined links point at the loopback receiver, the release binary has no
    // test-origin exception and the container has no egress: it must refuse and change nothing.
    let redo = uuid::Uuid::new_v4().to_string();
    let redo_preview_args = [
        "post-relay",
        "preview",
        "--action",
        "redeliver",
        "--profile-id",
        cpid.as_str(),
        "--operation",
        redo.as_str(),
        "--operator",
        "t1-image-operator",
        "--reason",
        "t1 release image",
    ];
    let redo_preview = img.run(&redo_preview_args).await;
    assert!(
        redo_preview.ok,
        "the read-only redeliver preview succeeds: {}",
        redo_preview.stderr
    );
    let redo_confirm = last_json(&redo_preview.stdout)["confirmation"]
        .as_str()
        .expect("a real confirmation value from the preview")
        .to_string();
    let redo_args = [
        "post-relay",
        "redeliver",
        "--profile-id",
        cpid.as_str(),
        "--operation",
        redo.as_str(),
        "--operator",
        "t1-image-operator",
        "--reason",
        "t1 release image",
        "--confirm",
        redo_confirm.as_str(),
    ];
    let r = img.run(&redo_args).await;
    assert!(
        !r.ok,
        "the release image refuses a non-allowlisted loopback destination: {}{}",
        r.stdout, r.stderr
    );
    assert_eq!(db.count("post_relay_dead_letters").await, 5);
    assert_eq!(
        db.i64(&format!(
            "SELECT count(*) FROM post_relay_bodies WHERE profile_id={}",
            c.profile_id
        ))
        .await,
        5
    );
    assert_eq!(
        db.i64(&format!(
            "SELECT count(*) FROM post_relay_operations WHERE id='{redo}'"
        ))
        .await,
        0,
        "refused before any audit or staging"
    );

    // The live worker was never disturbed: every live request reached the receiver exactly once.
    let sent = live.await.unwrap();
    assert_eq!(sent, 60);
    eventually(
        "live traffic delivered",
        Duration::from_secs(60),
        || async {
            w.receiver
                .accepted()
                .iter()
                .filter(|x| String::from_utf8_lossy(&x.raw_body).contains("live "))
                .count()
                == 60
        },
    )
    .await;
    assert_eq!(
        w.receiver.attempts().len() - attempts_before,
        60,
        "no attempt came from the image"
    );
    for text in [
        &p.stdout, &p.stderr, &d.stdout, &d.stderr, &r.stdout, &r.stderr, &s.stdout, &s.stderr,
    ] {
        assert!(
            !text.contains("BEGIN")
                && !text.contains("raw_body")
                && !text
                    .contains(&String::from_utf8_lossy(super::emumet::MASTER_PASSWORD).to_string()),
            "{text}"
        );
    }
    eventually("live bodies purged", Duration::from_secs(30), || async {
        db.i64("SELECT body_count FROM post_relay_quota").await == 5
    })
    .await;
    let remaining = db.i64("SELECT body_count FROM post_relay_quota").await;
    assert_eq!(
        remaining, 5,
        "only the five refused-redelivery bodies remain reserved"
    );
    let _ = docker(&["volume", "rm", "-f", &volume].map(String::from), None).await;
}
