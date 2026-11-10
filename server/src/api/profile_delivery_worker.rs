//! Profile 配送 worker の常駐ループ (P3o)。
//!
//! 本番の `main` からは起動しない (P4 が root から起動する)。テストと P4 が使う部品として置く。
//! 1 サイクルの処理 (lease・署名・送信・再試行) は `application::service::profile_relationship`。

use crate::handler::AppModule;
use application::service::profile_relationship::{
    DeliverProfileActivitiesUseCase, ProfileDeliveryEnv,
};
use kernel::prelude::entity::ProfileDeliveryPolicy;
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use tokio::sync::watch;

#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct ProfileDeliveryShutdown {
    tx: watch::Sender<bool>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl ProfileDeliveryShutdown {
    pub fn trigger(&self) {
        let _ = self.tx.send(true);
    }
}

/// 配送ループを起動する。`interval` ごとに 1 サイクル回す。
#[cfg_attr(not(test), allow(dead_code))]
pub fn spawn_profile_delivery_worker(
    module: Arc<AppModule>,
    policy: ProfileDeliveryPolicy,
    interval: Duration,
) -> (ProfileDeliveryShutdown, tokio::task::JoinHandle<()>) {
    let (tx, rx) = watch::channel(false);
    let handle = tokio::spawn(run(module, policy, interval, rx));
    (ProfileDeliveryShutdown { tx }, handle)
}

async fn run(
    module: Arc<AppModule>,
    policy: ProfileDeliveryPolicy,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let worker_id = format!("{}-{}", std::process::id(), uuid::Uuid::new_v4());
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _ = ticker.tick() => {
                let clock = OffsetDateTime::now_utc;
                let jitter = || rand::random::<f64>();
                let stop = || *shutdown.borrow();
                let env = ProfileDeliveryEnv {
                    clock: &clock,
                    jitter: &jitter,
                    should_stop: &stop,
                };
                match module.deliver_profile_activities(&worker_id, &policy, &env).await {
                    Ok(report) => {
                        if report.attempted > 0 {
                            tracing::info!(
                                attempted = report.attempted,
                                delivered = report.delivered,
                                retrying = report.retrying,
                                failed = report.failed,
                                cancelled = report.cancelled,
                                domain_blocked = report.domain_blocked,
                                lease_lost = report.lease_lost,
                                "profile delivery cycle"
                            );
                        }
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "profile delivery cycle failed");
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
