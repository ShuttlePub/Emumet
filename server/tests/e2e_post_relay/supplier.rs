//! Runs e2e/post-relay/capacity-supplier.sh (the real supplier) against a test database.
use std::process::Stdio;
use tokio::process::{Child, Command};

pub struct Supplier {
    child: Option<Child>,
    database_url: String,
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
    /// A single failed sample is tolerated by the design (the periodic supplier simply reports
    /// again next period and nothing invented is stored), so a loaded CI runner may need a retry;
    /// every attempt's stderr is kept so a persistent failure is diagnosable.
    pub async fn once(database_url: &str) {
        let mut failures = Vec::new();
        for attempt in 1..=5 {
            let out = Self::command(database_url, 30, true)
                .output()
                .await
                .unwrap();
            if out.status.success() && out.stderr.is_empty() {
                if !failures.is_empty() {
                    eprintln!("supplier sample needed {attempt} attempts: {failures:?}");
                }
                return;
            }
            failures.push(format!(
                "attempt {attempt}: status {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            ));
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        panic!("supplier sample failed 5 times: {failures:#?}");
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
        let child = Self::command(database_url, 30, false)
            .env("SUPPLIER_SKIP_FIRST", "1")
            .spawn()
            .unwrap();
        Self {
            child: Some(child),
            database_url: database_url.into(),
        }
    }
    pub async fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill().await;
            let _ = c.wait().await;
        }
    }
    pub fn database_url(&self) -> &str {
        &self.database_url
    }
}
