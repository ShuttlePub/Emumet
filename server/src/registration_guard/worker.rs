use crate::handler::AppModule;
use application::service::registration_protection::RegistrationProtectionReadinessUseCase;
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

pub(super) async fn run(
    module: Arc<AppModule>,
    period: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let Ok(interval) = time::Duration::try_from(period) else {
        tracing::error!("registration maintenance period is out of range");
        return;
    };
    let mut ticker = tokio::time::interval(period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _ = ticker.tick() => {
                // Includes the transaction-owned lock. Cancellation drops/rolls back the
                // reservation; no sticky lease can survive a dead process.
                tokio::select! {
                    result = tokio::time::timeout(Duration::from_secs(20),
                        module.run_registration_protection_maintenance(interval)) => {
                        if !matches!(result, Ok(Ok(_))) {
                            // Never log provider errors, request bodies, secrets or PII.
                            tracing::warn!("registration protection maintenance did not complete");
                        }
                    }
                    _ = shutdown.changed() => break,
                }
            }
            _ = shutdown.changed() => break,
        }
    }
}
