use crate::handler::AppModule;
use application::service::registration_enforcement_probe::RegistrationEnforcementMonitorUseCase;
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

pub(super) async fn run(module: Arc<AppModule>, shutdown: watch::Receiver<bool>) {
    let period = std::env::var("REGISTRATION_ENFORCEMENT_PROBE_PERIOD_SECS")
        .ok()
        .map(|value| value.parse::<u64>().ok())
        .unwrap_or(Some(30))
        .filter(|secs| (1..=300).contains(secs));
    let Some(period) = period else {
        let _ = module.invalidate_registration_enforcement().await;
        tracing::error!("invalid registration enforcement probe period; registration stopped");
        return;
    };
    tokio::join!(
        probe(
            module.clone(),
            Duration::from_secs(period),
            shutdown.clone()
        ),
        freshness(module, shutdown),
    );
}

async fn probe(module: Arc<AppModule>, period: Duration, mut shutdown: watch::Receiver<bool>) {
    let interval = time::Duration::try_from(period).expect("bounded seconds");
    let mut ticker = tokio::time::interval(period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _ = ticker.tick() => {
                tokio::select! {
                    result = tokio::time::timeout(Duration::from_secs(20),
                        module.run_registration_enforcement_probe(interval)) => {
                        if !matches!(result, Ok(Ok(_))) {
                            let _ = module.invalidate_registration_enforcement().await;
                            tracing::warn!("registration enforcement negative canary did not complete");
                        }
                    }
                    _ = shutdown.changed() => break,
                }
            }
            _ = shutdown.changed() => break,
        }
    }
}

async fn freshness(module: Arc<AppModule>, mut shutdown: watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(5));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _ = ticker.tick() => {
                if !matches!(tokio::time::timeout(Duration::from_secs(2),
                    module.refresh_registration_enforcement()).await, Ok(Ok(_))) {
                    tracing::warn!("registration enforcement freshness could not be persisted");
                }
            }
            _ = shutdown.changed() => break,
        }
    }
}
