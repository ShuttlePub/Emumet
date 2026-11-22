//! Dedicated E2E stack (e2e/post-relay/compose.yml): ports, docker control and health waits.
use std::process::Command;
use std::time::Duration;

pub const PG: &str = "postgres://postgres:develop@127.0.0.1:15450/postgres";
pub const KRATOS_PUBLIC: &str = "http://127.0.0.1:14433";
pub const KRATOS_ADMIN: &str = "http://127.0.0.1:14434";
pub const HYDRA_PUBLIC: &str = "http://127.0.0.1:14444";
pub const HYDRA_ADMIN: &str = "http://127.0.0.1:14445";
pub const KETO_READ: &str = "http://127.0.0.1:14466";
pub const KETO_WRITE: &str = "http://127.0.0.1:14467";
/// The Hydra login/consent redirect (e2e/post-relay/hydra.yml) points here.
pub const LOGIN_ADDR: &str = "127.0.0.1:18080";
/// HTTPS origin the Emumet under test advertises (the inbox requires a normalized HTTPS origin).
/// It is never resolved: requests are made to the real loopback listener.
pub const PUBLIC: &str = "https://emumet.e2e.example";
pub const COMPOSE_PROJECT: &str = "emumet-post-relay-e2e";

pub fn compose_file() -> String {
    format!(
        "{}/../e2e/post-relay/compose.yml",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Makes DATABASE_URL point at the stack's PostgreSQL unless the caller already chose one.
pub fn ensure_database_url() -> String {
    match std::env::var("DATABASE_URL") {
        Ok(v) => v,
        Err(_) => {
            std::env::set_var("DATABASE_URL", PG);
            PG.into()
        }
    }
}

pub fn compose(args: &[&str]) -> std::process::Output {
    let output = Command::new("docker")
        .arg("compose")
        .args(["-p", COMPOSE_PROJECT, "-f", &compose_file()])
        .args(args)
        .output()
        .expect("docker is required for the T1 E2E");
    assert!(
        output.status.success(),
        "docker compose {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

pub async fn wait_http(url: &str, what: &str, bound: Duration) {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let end = std::time::Instant::now() + bound;
    loop {
        if client
            .get(url)
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            return;
        }
        assert!(
            std::time::Instant::now() < end,
            "{what} did not become ready ({url})"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

pub async fn wait_ory() {
    wait_http(
        &format!("{KRATOS_PUBLIC}/health/alive"),
        "Kratos",
        Duration::from_secs(90),
    )
    .await;
    wait_http(
        &format!("{HYDRA_PUBLIC}/health/alive"),
        "Hydra",
        Duration::from_secs(90),
    )
    .await;
    wait_http(
        &format!("{KETO_READ}/health/alive"),
        "Keto",
        Duration::from_secs(90),
    )
    .await;
}

/// Real Ory stop/start/pause used by the outage cases.
pub fn ory_stop() {
    compose(&["stop", "kratos", "hydra", "keto"]);
}
pub fn ory_start() {
    compose(&["start", "kratos", "hydra", "keto"]);
}
/// A paused container still accepts TCP connections (kernel backlog) but never answers: the
/// "responds-but-stalls" Ory failure mode.
pub fn ory_pause() {
    compose(&["pause", "kratos", "hydra", "keto"]);
}
pub fn ory_unpause() {
    compose(&["unpause", "kratos", "hydra", "keto"]);
}
