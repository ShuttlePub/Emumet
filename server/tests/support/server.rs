use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};

const SERVER_ADDR: &str = "127.0.0.1:8080";

pub struct EmumetServer {
    child: Option<Child>,
}

impl EmumetServer {
    /// Start the server.
    ///
    /// If `EMUMET_E2E_EXTERNAL_SERVER=1` is set, skip spawning and wait for an
    /// already-running server on `SERVER_ADDR`.  Otherwise spawn via
    /// `cargo run -p server` as before.
    pub async fn start() -> Self {
        if is_external_server() {
            Self::start_external().await
        } else {
            Self::start_inner(None, &[]).await
        }
    }

    /// Start the server with AP test environment overrides.
    ///
    /// When `EMUMET_E2E_EXTERNAL_SERVER=1` is set, delegates to
    /// [`start_with_ap_test_external`]; otherwise spawns the server
    /// with test-mode features and env overrides.
    pub async fn start_with_ap_test(mock_peer_host: &str) -> Self {
        if is_external_server() {
            Self::start_with_ap_test_external(mock_peer_host).await
        } else {
            let env_overrides: &[(&str, &str)] = &[
                ("AP_TEST_ALLOWED_FETCH_HOSTS", mock_peer_host),
                ("AP_TEST_ACCEPT_INVALID_CERTS", "1"),
            ];
            Self::start_inner(Some("test-mode"), env_overrides).await
        }
    }

    /// Wait for an already-running server on [`SERVER_ADDR`] (external mode).
    ///
    /// This is a convenience wrapper used when `EMUMET_E2E_EXTERNAL_SERVER=1`.
    /// The returned handle owns no child process and performs no cleanup.
    pub async fn start_external() -> Self {
        let mut server = Self { child: None };
        server.wait_until_ready().await;
        server
    }

    /// Wait for an already-running server on [`SERVER_ADDR`] with AP test
    /// expectations (external mode).
    ///
    /// Like [`start_external`], this skips spawning.  The caller must have
    /// started the server externally with `test-mode` features and the
    /// `AP_TEST_ALLOWED_FETCH_HOSTS` / `AP_TEST_ACCEPT_INVALID_CERTS` vars.
    pub async fn start_with_ap_test_external(_mock_peer_host: &str) -> Self {
        // In external mode the server was pre-started with the right env.
        Self::start_external().await
    }

    async fn start_inner(features: Option<&str>, env_overrides: &[(&str, &str)]) -> Self {
        dotenvy::dotenv().ok();

        let root_dir = workspace_root();

        let mut command = Command::new("cargo");
        command
            .arg("run")
            .arg("-p")
            .arg("server")
            .current_dir(root_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        if let Some(f) = features {
            command.arg("--features").arg(f);
        }

        for key in required_env_keys() {
            if let Ok(value) = std::env::var(key) {
                command.env(key, value);
            }
        }

        command.env(
            "RUST_LOG",
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
        );

        command.env("PUBLIC_BASE_URL", format!("http://{SERVER_ADDR}"));

        for (key, val) in env_overrides {
            command.env(key, val);
        }

        let child = command
            .spawn()
            .expect("failed to spawn `cargo run -p server`");
        let mut server = Self { child: Some(child) };
        server.wait_until_ready().await;
        server
    }

    async fn wait_until_ready(&mut self) {
        let ready = timeout(Duration::from_secs(60), async {
            loop {
                // In external mode there is no child to poll.
                if let Some(ref mut child) = self.child {
                    if child
                        .try_wait()
                        .expect("failed to poll server process")
                        .is_some()
                    {
                        panic!("server process exited before becoming ready");
                    }
                }

                if TcpStream::connect(SERVER_ADDR).await.is_ok() {
                    break;
                }
                sleep(Duration::from_millis(300)).await;
            }
        })
        .await;

        if ready.is_err() {
            panic!("timed out waiting for server at {SERVER_ADDR}");
        }
    }
}

impl Drop for EmumetServer {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            if child.id().is_some() {
                let _ = child.start_kill();
            }
        }
    }
}

fn is_external_server() -> bool {
    std::env::var("EMUMET_E2E_EXTERNAL_SERVER").as_deref() == Ok("1")
}

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("server crate should be under workspace root")
        .to_path_buf()
}

fn required_env_keys() -> &'static [&'static str] {
    &[
        "DATABASE_URL",
        "HYDRA_ISSUER_URL",
        "HYDRA_ADMIN_URL",
        "KRATOS_PUBLIC_URL",
        "EXPECTED_AUDIENCE",
        "KETO_READ_URL",
        "KETO_WRITE_URL",
        "REDIS_URL",
        "REDIS_HOST",
        "WORKER_ID",
    ]
}
