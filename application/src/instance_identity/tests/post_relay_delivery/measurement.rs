//! Opt-in final implementation measurements. Production transport/orchestration; loopback-only
//! test-mode exception, plain HTTP, real Q1 DB/immutable synthetic envelope bytes.
use super::*;
use driver::{home_origin::HomeOriginAllowlist, post_relay_delivery::HttpPostRelayTransport};
async fn perf_setup(slow: bool) -> (Env, TestModule, Vec<Profile>, Vec<LocalReceiver>) {
    let env = Env::new().await;
    let (mut m, _) = env.process();
    m.ensure_instance_identity().await.unwrap();
    let (_, profiles) = fixture(&m.database, 16, true).await;
    let mut peers = Vec::new();
    for i in 0..4 {
        peers.push(
            LocalReceiver::start(if slow && i == 1 {
                Duration::from_secs(30)
            } else {
                Duration::ZERO
            })
            .await,
        );
    }
    let origins: Vec<String> = peers.iter().map(LocalReceiver::uri).collect();
    for (i, p) in profiles.iter().enumerate() {
        sqlx::query("UPDATE profile_links SET origin=$2 WHERE profile_id=$1")
            .bind(p.id().as_ref())
            .bind(&origins[i % 4])
            .execute(&env.iso.pool)
            .await
            .unwrap();
    }
    m.transport.production = Some(
        HttpPostRelayTransport::new(HomeOriginAllowlist::from_json("[]").unwrap())
            .with_test_origins(origins),
    );
    (env, m, profiles, peers)
}
async fn batch(
    worker: &Arc<RelayDelivery<TestModule>>,
    m: &TestModule,
    profiles: &[Profile],
    n: usize,
    bytes: usize,
    parallel: usize,
    prefix: &str,
) -> Duration {
    for i in 0..n {
        admit_payload(
            m,
            &profiles[i % profiles.len()],
            &format!("{prefix}-{i}"),
            false,
            vec![b'x'; bytes],
        )
        .await;
    }
    let start = Instant::now();
    let mut done = 0;
    while done < n {
        let leases = worker.claim(parallel as u16).await.unwrap();
        assert!(!leases.is_empty());
        let mut tasks = tokio::task::JoinSet::new();
        for lease in leases {
            let w = worker.clone();
            tasks.spawn(async move { w.deliver(lease).await });
        }
        while let Some(result) = tasks.join_next().await {
            assert_eq!(result.unwrap().unwrap(), Recorded::Applied);
            done += 1;
        }
    }
    start.elapsed()
}
fn rss() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|s| s.starts_with("VmRSS:"))
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}
fn cpu() -> u64 {
    std::fs::read_to_string("/proc/self/stat")
        .unwrap()
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .skip(11)
        .take(2)
        .map(|s| s.parse::<u64>().unwrap())
        .sum()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit release benchmark; 10-minute steady-state and live PostgreSQL required"]
async fn s1_final_real_http_release_measurement() {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let limits = std::fs::read_to_string("/proc/self/limits").unwrap();
    eprintln!(
        "S1_ENV {} {} external_receiver_cores={:?}",
        status
            .lines()
            .find(|l| l.starts_with("Cpus_allowed_list"))
            .unwrap_or("unknown affinity"),
        limits
            .lines()
            .find(|l| l.starts_with("Max address space"))
            .unwrap_or("unknown AS limit"),
        std::env::var("S1_EXTERNAL_RECEIVER_CORES")
    );
    assert!(std::env::var("DATABASE_URL").is_ok());
    for bytes in [65536, 1_400_000] {
        for parallel in [1usize, 16] {
            let (env, m, profiles, _peers) = perf_setup(false).await;
            let worker = Arc::new(RelayDelivery::new(Arc::new(m.clone())));
            let before = cpu();
            let r0 = rss();
            let elapsed = batch(&worker, &m, &profiles, 128, bytes, parallel, "speed").await;
            eprintln!("S1_FINAL bytes={bytes} parallel={parallel} n=128 seconds={:.3} rps={:.2} cpu_ticks={} rss_before_kib={r0} rss_after_kib={} kdf_calls={} kdf_ms={:.3} timings={:?}",elapsed.as_secs_f64(),128.0/elapsed.as_secs_f64(),cpu()-before,rss(),m.key_encryptor.decrypts.load(Ordering::SeqCst),m.key_encryptor.decrypt_ns.load(Ordering::Relaxed) as f64/1e6,worker.metrics.snapshot());
            let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM post_relay_bodies")
                .fetch_one(&env.iso.pool)
                .await
                .unwrap();
            assert_eq!(rows, 0);
        }
    }
    let (env, m, profiles, _peers) = perf_setup(false).await;
    let worker = Arc::new(RelayDelivery::new(Arc::new(m.clone())));
    let started = Instant::now();
    let mut rounds = 0u64;
    let mut next = Duration::ZERO;
    while started.elapsed() < Duration::from_secs(600) {
        batch(
            &worker,
            &m,
            &profiles,
            64,
            65536,
            16,
            &format!("steady-{rounds}"),
        )
        .await;
        rounds += 1;
        worker.maintain().await.unwrap();
        if started.elapsed() >= next {
            let (rows,connections,waiting):(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM post_relay_bodies),(SELECT count(*) FROM pg_stat_activity WHERE datname=current_database()),(SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock')").fetch_one(&env.iso.pool).await.unwrap();
            assert_eq!(
                rows, 0,
                "a completed batch must not retain failed/retry bodies"
            );
            let counts: serde_json::Value = sqlx::query_scalar(
                r#"SELECT jsonb_build_object(
                'bodies',(SELECT count(*) FROM post_relay_bodies),
                'jobs',(SELECT count(*) FROM post_relay_jobs),
                'attempts',(SELECT count(*) FROM post_relay_attempts),
                'dead_letters',(SELECT count(*) FROM post_relay_dead_letters),
                'audits',(SELECT count(*) FROM post_relay_audits),
                'operations',(SELECT count(*) FROM post_relay_operations),
                'unlinked',(SELECT count(*) FROM post_relay_unlinked),
                'schedule',(SELECT count(*) FROM post_relay_schedule),
                'slots',(SELECT count(*) FROM post_relay_slots),
                'capacity',(SELECT count(*) FROM post_relay_capacity),
                'quota',(SELECT count(*) FROM post_relay_quota),
                'profile_quota',(SELECT count(*) FROM post_relay_profile_quota),
                'link_usage',(SELECT count(*) FROM post_relay_link_usage),
                'counters',(SELECT count(*) FROM post_relay_counters))"#,
            )
            .fetch_one(&env.iso.pool)
            .await
            .unwrap();
            eprintln!("S1_STEADY seconds={:.1} succeeded={} rss_kib={} cpu_ticks={} live_bodies={rows} connections={connections} lock_waiters={waiting} kdf_calls={}",started.elapsed().as_secs_f64(),rounds*64,rss(),cpu(),m.key_encryptor.decrypts.load(Ordering::SeqCst));
            eprintln!(
                "S1_TABLE_COUNTS seconds={:.1} counts={counts}",
                started.elapsed().as_secs_f64()
            );
            next += Duration::from_secs(60);
        }
    }
    eprintln!(
        "S1_STEADY_END seconds={:.2} n={} rate_including_admission={:.2} metrics={:?}",
        started.elapsed().as_secs_f64(),
        rounds * 64,
        (rounds * 64) as f64 / started.elapsed().as_secs_f64(),
        worker.metrics.snapshot()
    );
    assert_eq!(m.key_encryptor.decrypts.load(Ordering::SeqCst), 1);
}

// wiremock 0.6.5 retains matched request bodies in MountedMock even when global request
// recording is disabled. A streaming TCP HTTP recipient is required for meaningful RSS.
use tokio::io::{AsyncReadExt, AsyncWriteExt};
struct ReceiverChild(std::process::Child);
impl Drop for ReceiverChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct LocalReceiver {
    address: std::net::SocketAddr,
    accept: Option<tokio::task::JoinHandle<()>>,
    _child: Option<ReceiverChild>,
    // Keep the pipe open: libtest writes its long-running-test warning after 60 seconds.
    // Dropping it at READY makes a healthy receiver exit on EPIPE during a 10-minute run.
    _child_stdout: Option<std::io::BufReader<std::process::ChildStdout>>,
}
impl Drop for LocalReceiver {
    fn drop(&mut self) {
        if let Some(h) = &self.accept {
            h.abort();
        }
    }
}
impl LocalReceiver {
    fn uri(&self) -> String {
        format!("http://{}", self.address)
    }
    async fn start(delay: Duration) -> Self {
        if let Ok(cores) = std::env::var("S1_EXTERNAL_RECEIVER_CORES") {
            use std::io::BufRead;
            let mut child=ReceiverChild(std::process::Command::new("taskset").args(["-c",&cores]).arg(std::env::current_exe().unwrap()).args(["--exact","instance_identity::tests::post_relay_delivery::measurement::s1_streaming_receiver_process","--ignored","--nocapture"]).env_remove("S1_EXTERNAL_RECEIVER_CORES").env("S1_RECEIVER_SUBPROCESS","1").env("S1_RECEIVER_DELAY_MS",delay.as_millis().to_string()).stdout(std::process::Stdio::piped()).spawn().unwrap());
            let mut output = std::io::BufReader::new(child.0.stdout.take().unwrap());
            let mut line = String::new();
            loop {
                line.clear();
                if output.read_line(&mut line).unwrap() == 0 {
                    panic!("recipient subprocess failed before listening");
                }
                if let Some(a) = line.trim_end().strip_prefix("S1_RECEIVER_READY ") {
                    return Self {
                        address: a.parse().unwrap(),
                        accept: None,
                        _child: Some(child),
                        _child_stdout: Some(output),
                    };
                }
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut header = Vec::with_capacity(2048);
                    let mut buf = [0u8; 8192];
                    let end = loop {
                        let Ok(n) = socket.read(&mut buf).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        header.extend_from_slice(&buf[..n]);
                        if let Some(i) = header.windows(4).position(|v| v == b"\r\n\r\n") {
                            break i + 4;
                        }
                        if header.len() > 16384 {
                            return;
                        }
                    };
                    let text = std::str::from_utf8(&header[..end]).unwrap();
                    let length: usize = text
                        .lines()
                        .find_map(|l| {
                            l.split_once(':')
                                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                                .map(|(_, v)| v.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    let mut left = length.saturating_sub(header.len() - end);
                    drop(header);
                    while left > 0 {
                        let room = left.min(buf.len());
                        let Ok(n) = socket.read(&mut buf[..room]).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        left -= n;
                    }
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    let _=socket.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\nContent-Length: 0\r\n\r\n").await;
                });
            }
        });
        Self {
            address,
            accept: Some(accept),
            _child: None,
            _child_stdout: None,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit release origin isolation measurement; PostgreSQL required"]
async fn s1_origin_timeout_isolation_release_measurement() {
    let mut values = [Vec::new(), Vec::new()];
    for round in 0..3 {
        for mixed in [false, true] {
            let (_env, m, profiles, _peers) = perf_setup(mixed).await;
            let worker = Arc::new(RelayDelivery::new(Arc::new(m.clone())));
            let _ = m.unlock_instance_signing_key().await.unwrap();
            for i in 0..512 {
                let p = &profiles[(i % 4) * 4];
                admit_payload(&m, p, &format!("healthy-{i}"), false, vec![b'x'; 65536]).await;
            }
            if mixed {
                for i in 0..16 {
                    let p = &profiles[(i % 4) * 4 + 1];
                    admit_payload(&m, p, &format!("slow-{i}"), false, vec![b'x'; 65536]).await;
                }
            }
            let start = Instant::now();
            let mut done = 0usize;
            let mut admitted = 512usize;
            let mut slow_completed = 0usize;
            let mut tasks = tokio::task::JoinSet::new();
            // Continue beyond the absolute ten-second deadline: measuring only the initial
            // 512 successes would abort slow requests before any real timeout occurred.
            while done < 512 || start.elapsed() < Duration::from_secs(12) {
                if admitted - done < 128 {
                    for _ in 0..128 {
                        let p = &profiles[(admitted % 4) * 4];
                        admit_payload(
                            &m,
                            p,
                            &format!("healthy-{admitted}"),
                            false,
                            vec![b'x'; 65536],
                        )
                        .await;
                        admitted += 1;
                    }
                }
                if tasks.len() < 16 {
                    for lease in worker.claim((16 - tasks.len()) as u16).await.unwrap() {
                        let healthy = lease.receipt.starts_with("healthy-");
                        let w = worker.clone();
                        tasks.spawn(async move { (healthy, w.deliver(lease).await) });
                    }
                }
                let (healthy, result) = tasks
                    .join_next()
                    .await
                    .expect("scheduler starved the healthy origin")
                    .unwrap();
                assert_eq!(result.unwrap(), Recorded::Applied);
                if healthy {
                    done += 1;
                } else {
                    slow_completed += 1;
                }
            }
            let elapsed = start.elapsed();
            assert!(
                !mixed || slow_completed >= 4,
                "real origin timeouts were not observed"
            );
            tasks.abort_all();
            let rps = done as f64 / elapsed.as_secs_f64();
            values[mixed as usize].push(rps);
            eprintln!("S1_ISOLATION round={round} mixed={mixed} healthy_n={done} slow_timeouts={slow_completed} seconds={:.3} healthy_rps={rps:.2}",elapsed.as_secs_f64());
        }
    }
    values[0].sort_by(f64::total_cmp);
    values[1].sort_by(f64::total_cmp);
    let ratio = values[1][1] / values[0][1];
    eprintln!(
        "S1_ISOLATION_MEDIAN alone={:.2} mixed={:.2} ratio={ratio:.4} target=0.8",
        values[0][1], values[1][1]
    );
    assert!(ratio >= 0.8, "healthy origin isolation target missed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "subprocess recipient helper, only active with explicit S1_RECEIVER_SUBPROCESS=1"]
async fn s1_streaming_receiver_process() {
    if std::env::var("S1_RECEIVER_SUBPROCESS").as_deref() != Ok("1") {
        return;
    }
    let delay = Duration::from_millis(
        std::env::var("S1_RECEIVER_DELAY_MS")
            .unwrap()
            .parse()
            .unwrap(),
    );
    let receiver = LocalReceiver::start(delay).await;
    println!("S1_RECEIVER_READY {}", receiver.address);
    std::future::pending::<()>().await;
}
