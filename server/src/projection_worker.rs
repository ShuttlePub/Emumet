use crate::handler::AppModule;
use application::projection::ProjectAccountBatch;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Poll-driven tailing worker for the account projection (ADR 0006 Stage 3).
/// Runs `project_batch` once per interval and stops on shutdown trigger.
pub struct ProjectionWorker {
    module: Arc<AppModule>,
    interval: Duration,
    shutdown: watch::Receiver<bool>,
}

/// Cooperative shutdown handle for the worker.
#[derive(Clone)]
pub struct ProjectionShutdown {
    tx: watch::Sender<bool>,
}

impl ProjectionShutdown {
    pub fn trigger(&self) {
        let _ = self.tx.send(true);
    }
}

impl ProjectionWorker {
    pub fn spawn(module: Arc<AppModule>, interval: Duration) -> (JoinHandle<()>, ProjectionShutdown) {
        let (tx, rx) = watch::channel(false);
        let worker = Self {
            module,
            interval,
            shutdown: rx,
        };
        let handle = tokio::spawn(worker.run());
        (handle, ProjectionShutdown { tx })
    }

    async fn run(mut self) {
        let mut ticker = tokio::time::interval(self.interval);
        loop {
            if *self.shutdown.borrow() {
                break;
            }
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(error) = self.module.project_batch().await {
                        tracing::error!(error = %error, "projection tailing batch failed");
                    }
                }
                _ = self.shutdown.changed() => {
                    if *self.shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    }
}

/// Parse `PROJECTION_POLL_INTERVAL_MS` (default 100ms; e2e-compatible latency).
pub fn projection_poll_interval_from_env() -> Duration {
    let millis: u64 = std::env::var("PROJECTION_POLL_INTERVAL_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(100);
    Duration::from_millis(millis)
}
