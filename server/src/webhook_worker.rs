use crate::handler::AppModule;
use application::service::registration_gate_webhook::{
    DeliverRegistrationGateWebhooksUseCase, ImportEnvironmentWebhookEndpointUseCase,
};
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::repository::{
    DependOnWebhookDeliveryRepository, DependOnWebhookSender, WebhookDeliveryRepository,
    WebhookSender,
};
use kernel::prelude::entity::{WebhookConfigStatus, WebhookDeliveryPolicy};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// Cooperative shutdown handle for the webhook delivery worker.
#[derive(Clone)]
pub struct WebhookShutdown {
    tx: watch::Sender<bool>,
}

impl WebhookShutdown {
    pub fn trigger(&self) {
        let _ = self.tx.send(true);
    }
}

/// Parse `REGISTRATION_WEBHOOK_POLL_INTERVAL_MS` (default 2s).
fn poll_interval_from_env() -> Duration {
    let millis: u64 = std::env::var("REGISTRATION_WEBHOOK_POLL_INTERVAL_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|millis| *millis > 0)
        .unwrap_or(2000);
    Duration::from_millis(millis)
}

/// Import the environment-variable destination (only when an encryption key is configured) and
/// establish the enable-time baselines before startup producers can emit new gate events.
/// This performs no external delivery; a spawned worker alone has no scheduling-order guarantee.
pub async fn prepare_baseline(module: &AppModule) -> error_stack::Result<(), kernel::KernelError> {
    match module.import_environment_webhook_endpoint().await {
        Ok(Some(_)) => tracing::info!(
            "registration gate webhook: the environment destination was imported as a regular destination"
        ),
        Ok(None) => {}
        Err(error) => {
            tracing::error!(error = %error, "registration gate webhook: environment destination import failed; it will be retried at the next start");
        }
    }
    let mut connection = module.database_connection().connection().await?;
    let imported = module
        .webhook_delivery_repository()
        .environment_import_done(&mut connection)
        .await?;
    if !imported && module.webhook_sender().status().is_enabled() {
        module
            .webhook_delivery_repository()
            .ingest_gate_events(&mut connection, time::OffsetDateTime::now_utc(), 0, true)
            .await?;
    }
    Ok(())
}

/// Starts the delivery worker. It always runs: destinations are managed through the admin API, so
/// there may be deliveries to make even when the environment variables are unset. A cycle with no
/// destination does nothing, and a failing destination never affects the gate.
pub fn spawn(module: Arc<AppModule>) -> WebhookShutdown {
    match module.webhook_sender().status() {
        WebhookConfigStatus::Disabled => {
            tracing::info!("registration gate webhook: no environment destination configured");
        }
        WebhookConfigStatus::Misconfigured { issue } => {
            tracing::warn!(
                issue = ?issue,
                "registration gate webhook: the environment destination is misconfigured and is not used"
            );
        }
        WebhookConfigStatus::Enabled { host } => {
            tracing::info!(destination_host = %host, "registration gate webhook: environment destination configured");
        }
    }
    let (tx, rx) = watch::channel(false);
    tokio::spawn(run(module, poll_interval_from_env(), rx));
    WebhookShutdown { tx }
}

async fn run(module: Arc<AppModule>, interval: Duration, mut shutdown: watch::Receiver<bool>) {
    let worker_id = format!("{}-{}", std::process::id(), uuid::Uuid::new_v4());
    let policy = WebhookDeliveryPolicy::default();
    let jitter = || rand::random::<f64>();
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _ = ticker.tick() => {
                let stop = || *shutdown.borrow();
                match module
                    .deliver_registration_gate_webhooks(&worker_id, &policy, &jitter, &stop)
                    .await
                {
                    Ok(report) => {
                        if report.attempted > 0 || report.ingested > 0 {
                            tracing::info!(
                                ingested = report.ingested,
                                attempted = report.attempted,
                                delivered = report.delivered,
                                retrying = report.retrying,
                                failed = report.failed,
                                lease_lost = report.lease_lost,
                                "registration gate webhook delivery cycle"
                            );
                        }
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "registration gate webhook delivery cycle failed");
                    }
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }
        }
    }
}
