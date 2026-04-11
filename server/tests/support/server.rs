use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};

const SERVER_ADDR: &str = "127.0.0.1:8080";

pub struct EmumetServer {
    child: Child,
}

impl EmumetServer {
    pub async fn start() -> Self {
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

        for key in required_env_keys() {
            if let Ok(value) = std::env::var(key) {
                command.env(key, value);
            }
        }

        command.env(
            "RUST_LOG",
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
        );

        let child = command
            .spawn()
            .expect("failed to spawn `cargo run -p server`");
        let mut server = Self { child };
        server.wait_until_ready().await;
        server
    }

    async fn wait_until_ready(&mut self) {
        let ready = timeout(Duration::from_secs(30), async {
            loop {
                if self
                    .child
                    .try_wait()
                    .expect("failed to poll server process")
                    .is_some()
                {
                    panic!("server process exited before becoming ready");
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
        if self.child.id().is_some() {
            let _ = self.child.start_kill();
        }
    }
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
