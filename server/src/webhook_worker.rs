use crate::handler::AppModule;
use application::service::registration_gate_webhook::{
    DeliverRegistrationGateWebhooksUseCase, ImportEnvironmentWebhookEndpointUseCase,
};
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::repository::{
    DependOnWebhookDeliveryRepository, DependOnWebhookSender, WebhookDeliveryRepository,
    WebhookInFlight, WebhookSender,
};
use kernel::prelude::entity::{WebhookConfigStatus, WebhookDeliveryPolicy};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// How long process shutdown waits for in-flight deliveries to be recorded. A sender's total
/// timeout is 15s, so an in-flight send always finishes (and its result is recorded) within this.
pub const STOP_GRACE: Duration = Duration::from_secs(20);

/// Cooperative shutdown handle for the webhook delivery worker.
pub struct WebhookShutdown {
    tx: watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
}

impl WebhookShutdown {
    pub fn trigger(&self) {
        let _ = self.tx.send(true);
    }

    /// Requests a stop and waits (at most `grace`) for the worker to finish its in-flight
    /// deliveries, so that a send that already reached the receiver is also recorded and is not
    /// sent again after a restart. Returns whether the worker stopped within `grace`.
    pub async fn stop_and_wait(self, grace: Duration) -> bool {
        self.trigger();
        tokio::time::timeout(grace, self.handle).await.is_ok()
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
    let handle = tokio::spawn(run(module, poll_interval_from_env(), rx));
    WebhookShutdown { tx, handle }
}

pub(crate) async fn run(
    module: Arc<AppModule>,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let worker_id = Arc::new(format!("{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    let policy = Arc::new(WebhookDeliveryPolicy::default());
    // 送信先ごとの処理中ガード: サイクルを重ねて起動しても同じ送信先は同時に 1 つだけ処理される。
    // 遅い送信先のサイクルを待たずに、次の tick で正常な送信先の配送が進む。
    let in_flight = WebhookInFlight::new();
    let mut cycles = tokio::task::JoinSet::new();
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _ = ticker.tick() => {
                while cycles.try_join_next().is_some() {}
                let module = module.clone();
                let worker_id = worker_id.clone();
                let policy = policy.clone();
                let in_flight = in_flight.clone();
                let stop_rx = shutdown.clone();
                cycles.spawn(async move {
                    let jitter = || rand::random::<f64>();
                    let stop = || *stop_rx.borrow();
                    match module
                        .deliver_registration_gate_webhooks_guarded(
                            &worker_id, &policy, &jitter, &stop, &in_flight,
                        )
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
                });
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }
        }
    }
    while cycles.join_next().await.is_some() {}
}
