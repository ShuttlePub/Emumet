//! Explicit opt-in release baseline: Q1 DB ports plus a real plain-HTTP control sender.
use super::*;
use crate::crypto::{Rsa2048PreparedSignerFactory, Rsa2048RawGenerator};
use kernel::interfaces::crypto::{PreparedSignerFactory, RawKeyGenerator, SigningAlgorithm};
use std::sync::Arc;
use std::time::Instant;
use wiremock::{Mock, MockServer, ResponseTemplate};
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit release benchmark; DATABASE_URL required"]
async fn s1_baseline_real_http() {
    assert!(std::env::var("DATABASE_URL").is_ok(), "real DB required");
    let d = IsolatedDatabase::create().await;
    let db = d.database.clone();
    let (_, profiles) = fixture(&db, 16, true).await;
    for (i, p) in profiles.iter().enumerate() {
        sqlx::query("UPDATE profile_links SET origin=$2 WHERE profile_id=$1")
            .bind(p.id().as_ref())
            .bind(format!("https://home-{}.example", i % 4))
            .execute(&d.pool)
            .await
            .unwrap();
    }
    let peer = MockServer::builder()
        .disable_request_recording()
        .start()
        .await;
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&peer)
        .await;
    let raw = Rsa2048RawGenerator.generate_raw().unwrap();
    let signer: Arc<dyn kernel::interfaces::crypto::PreparedSigner> = Rsa2048PreparedSignerFactory
        .prepare(&raw.private_key_pem, SigningAlgorithm::Rsa2048)
        .unwrap()
        .into();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for size in [65536, 1_400_000] {
        for parallel in [1usize, 16] {
            let n = 128usize;
            let now = OffsetDateTime::now_utc();
            physical(&db, now).await;
            for i in 0..n {
                assert_eq!(
                    admit(
                        &db,
                        &profiles[i % 16],
                        &format!("b-{size}-{parallel}-{i}"),
                        now,
                        size
                    )
                    .await,
                    Admission::Queued
                );
            }
            let cpu_before=process_cpu_ticks();
            let rss_before=process_rss_kib();
            let started = Instant::now();
            let (mut db_ns, mut sign_ns, mut http_ns) = (0u128, 0u128, 0u128);
            let mut completed = 0usize;
            while completed < n {
                let leases = claim(&db, OffsetDateTime::now_utc(), parallel as u16).await;
                assert!(!leases.is_empty());
                let mut tasks = tokio::task::JoinSet::new();
                for lease in leases {
                    let db = db.clone();
                    let signer = signer.clone();
                    let client = client.clone();
                    let url = peer.uri();
                    tasks.spawn(async move {
                        let t = Instant::now();
                        let permit = loop { match authorize(&db, &lease, OffsetDateTime::now_utc()).await { Authorization::Permitted(p) => break p, Authorization::Busy => tokio::time::sleep(std::time::Duration::from_millis(1)).await, _ => panic!("baseline authorization refused") } };
                        let db1 = t.elapsed().as_nanos();
                        let t = Instant::now();
                        use sha2::{Digest, Sha256}; use base64::Engine;
                        let digest = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(&permit.envelope));
                        signer.sign(format!("(request-target): post /internal/v1/emumet/activities\nhost: home.example\ndate: {}\ndigest: SHA-256={digest}", httpdate::fmt_http_date(std::time::SystemTime::now())).as_bytes()).unwrap();
                        let sign = t.elapsed().as_nanos();
                        let t = Instant::now();
                        assert_eq!(client.post(url).body(permit.envelope).send().await.unwrap().status(), 204);
                        let http = t.elapsed().as_nanos();
                        let t = Instant::now();
                        assert_eq!(record(&db,&lease,OffsetDateTime::now_utc(),AttemptResult::Success).await,Recorded::Applied);
                        (db1+t.elapsed().as_nanos(),sign,http)
                    });
                }
                while let Some(r) = tasks.join_next().await {
                    let (a, b, c) = r.unwrap();
                    db_ns += a;
                    sign_ns += b;
                    http_ns += c;
                    completed += 1;
                }
            }
            let elapsed = started.elapsed().as_secs_f64();
            eprintln!("S1_RESOURCE cpu_ticks={} rss_before_kib={} rss_after_kib={}",process_cpu_ticks().saturating_sub(cpu_before),rss_before,process_rss_kib());
            let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM post_relay_bodies")
                .fetch_one(&d.pool)
                .await
                .unwrap();
            eprintln!("S1_BASELINE bytes={size} parallel={parallel} n={n} seconds={elapsed:.3} rps={:.2} db_ms_per={:.3} sign_ms_per={:.3} http_ms_per={:.3} live_bodies={rows} TLS=none KDF=not_in_control_path", n as f64/elapsed, db_ns as f64/n as f64/1e6,sign_ns as f64/n as f64/1e6,http_ns as f64/n as f64/1e6);
        }
    }
    consistent(&db, OffsetDateTime::now_utc()).await;
}

fn process_cpu_ticks()->u64{std::fs::read_to_string("/proc/self/stat").ok().and_then(|v|v.rsplit_once(')').map(|(_,s)|s.split_whitespace().skip(11).take(2).filter_map(|n|n.parse::<u64>().ok()).sum())).unwrap_or(0)}
fn process_rss_kib()->u64{std::fs::read_to_string("/proc/self/status").ok().and_then(|v|v.lines().find(|s|s.starts_with("VmRSS:")).and_then(|s|s.split_whitespace().nth(1)).and_then(|s|s.parse().ok())).unwrap_or(0)}
