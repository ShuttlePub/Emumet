//! Runs e2e/post-relay/capacity-supplier.sh (the real supplier) against a test database.
use std::process::Stdio;
use tokio::process::{Child, Command};

pub struct Supplier {
    child: Option<Child>,
    database_url: String,
    stats: Option<tempfile::TempDir>,
}

fn script() -> String {
    format!(
        "{}/../e2e/post-relay/capacity-supplier.sh",
        env!("CARGO_MANIFEST_DIR")
    )
}

impl Supplier {
    fn command(database_url: &str, interval: u32, once: bool) -> Command {
        let mut c = Command::new("bash");
        c.arg(script())
            .env("DATABASE_URL", database_url)
            .env("SERVER_CMD", env!("CARGO_BIN_EXE_server"))
            .env("SUPPLIER_INTERVAL_SECS", interval.to_string())
            .env("SUPPLIER_ONCE", if once { "1" } else { "0" })
            .env("HYDRA_ISSUER_URL", "http://127.0.0.1:9/")
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdin(Stdio::null())
            .kill_on_drop(true);
        c
    }
    /// One synchronous sample (the first admission sample must exist before traffic starts).
    /// No retries: the supplier uses the DB clock, so a future monitor-host timestamp cannot
    /// be hidden by retrying. The continuous supplier may still lose a sample and fail closed.
    pub async fn once(database_url: &str) {
        let out = Self::command(database_url, 30, true)
            .output()
            .await
            .unwrap();
        assert!(
            out.status.success() && out.stderr.is_empty(),
            "supplier warned: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    /// One sample with an explicit measurement ("FREE_BYTES TOTAL_BYTES" printed by the command),
    /// the way a managed-DB or multi-volume deployment would feed the same script.
    pub async fn report(database_url: &str, free: u64, total: u64) {
        let out = Self::command(database_url, 30, true)
            .env("SUPPLIER_MEASURE_CMD", format!("echo {free} {total}"))
            .output()
            .await
            .unwrap();
        assert!(
            out.status.success() && out.stderr.is_empty(),
            "report failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    /// A failing measurement: the script must report NOTHING (no invented healthy sample).
    pub async fn failing_measurement(database_url: &str) -> String {
        let out = Self::command(database_url, 30, true)
            .env("SUPPLIER_MEASURE_CMD", "false")
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stderr).into_owned()
    }
    /// The continuous supplier at the documented period (30 s).
    pub async fn start(database_url: &str) -> Self {
        Self::once(database_url).await;
        let stats = tempfile::tempdir().unwrap();
        let child = Self::command(database_url, 30, false)
            .env("SUPPLIER_STATS_FILE", stats.path().join("cycles"))
            .env("SUPPLIER_SKIP_FIRST", "1")
            .spawn()
            .unwrap();
        Self {
            child: Some(child),
            database_url: database_url.into(),
            stats: Some(stats),
        }
    }
    pub async fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill().await;
            let _ = c.wait().await;
        }
    }
    /// Cycles of the continuous supplier that took longer than its period (recorded by the script).
    pub fn overruns(&self) -> Vec<String> {
        let Some(dir) = &self.stats else {
            return Vec::new();
        };
        std::fs::read_to_string(dir.path().join("cycles"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.ends_with(" overrun"))
            .map(str::to_string)
            .collect()
    }
    pub fn database_url(&self) -> &str {
        &self.database_url
    }
}

/// Runs the supplier script (no database, no docker: stub measurement/clock/report commands) for
/// `run` and returns its stats lines `(start_ms, elapsed_ms, state)` and stderr.
async fn stub_run(
    interval: u32,
    work_secs: u32,
    run: std::time::Duration,
) -> (Vec<(u64, u64, String)>, String) {
    let dir = tempfile::tempdir().unwrap();
    let stats = dir.path().join("stats");
    let measure = dir.path().join("measure.sh");
    std::fs::write(
        &measure,
        format!("#!/bin/sh\nsleep {work_secs}\necho 50 100\n"),
    )
    .unwrap();
    std::fs::set_permissions(
        &measure,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let mut child = Command::new("bash")
        .arg(script())
        .env("DATABASE_URL", "postgres://unused")
        .env("SERVER_CMD", "true")
        .env("SUPPLIER_INTERVAL_SECS", interval.to_string())
        .env("SUPPLIER_MEASURE_CMD", measure.to_str().unwrap())
        .env("SUPPLIER_CLOCK_CMD", "echo 2026-01-01T00:00:00.000000Z")
        .env("SUPPLIER_STATS_FILE", &stats)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = child.stderr.take().unwrap();
    tokio::time::sleep(run).await;
    child.kill().await.unwrap();
    let mut text = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut stderr, &mut text)
        .await
        .unwrap();
    let lines = std::fs::read_to_string(&stats)
        .unwrap_or_default()
        .lines()
        .map(|l| {
            let f: Vec<&str> = l.split(' ').collect();
            (
                f[1].parse().unwrap(),
                f[2].parse().unwrap(),
                f[3].to_string(),
            )
        })
        .collect();
    (lines, text)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn capacity_supplier_keeps_a_fixed_period_deducting_work_time_and_records_overruns() {
    // Work of 2 s per cycle inside a 3 s period: starts stay 3 s apart (a sleep-after-work loop
    // would drift to 5 s) and nothing is an overrun.
    let (cycles, stderr) = stub_run(3, 2, std::time::Duration::from_millis(13_500)).await;
    assert!(cycles.len() >= 4, "{cycles:?}");
    assert!(
        cycles.iter().all(|c| c.2 == "ok" && c.1 < 3000),
        "{cycles:?}"
    );
    for pair in cycles.windows(2) {
        let delta = pair[1].0 - pair[0].0;
        assert!(
            (2800..=3300).contains(&delta),
            "period drifted to {delta} ms: {cycles:?}"
        );
    }
    assert!(!stderr.contains("overran"), "{stderr}");

    // Work (4 s) longer than the period (3 s): every cycle is observed and recorded as an overrun,
    // reported on stderr, and the next cycle starts immediately instead of waiting another period.
    let (cycles, stderr) = stub_run(3, 4, std::time::Duration::from_millis(14_000)).await;
    assert!(cycles.len() >= 3, "{cycles:?}");
    assert!(
        cycles.iter().all(|c| c.2 == "overrun" && c.1 > 3000),
        "{cycles:?}"
    );
    assert!(stderr.contains("overran its period"), "{stderr}");
    for pair in cycles.windows(2) {
        let delta = pair[1].0 - pair[0].0;
        assert!(
            (3900..=4600).contains(&delta),
            "an overrun cycle is followed immediately (delta {delta} ms): {cycles:?}"
        );
    }
}
