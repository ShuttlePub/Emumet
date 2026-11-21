//! Short committed DB phases, immutable bytes, one conservative monotonic deadline.
use crate::instance_identity::UnlockInstanceSigningKeyUseCase;
use base64::{engine::general_purpose::STANDARD, Engine};
use kernel::interfaces::database::{
    Connection, DatabaseConnection, DependOnDatabaseConnection, DependOnTransactionManager,
    TransactionManager,
};
use kernel::interfaces::post_relay::*;
use kernel::KernelError;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Semaphore, SemaphorePermit};
#[derive(Default)]
pub struct RelayMetrics {
    db_phase_ns: AtomicU64,
    db_slot_wait_ns: AtomicU64,
    db_calls: AtomicU64,
    connection_wait_ns: AtomicU64,
    lock_wait_ns: AtomicU64,
    scheduler_lock_wait_ns: AtomicU64,
    queued_jobs: AtomicU64,
    queue_wait_ns: AtomicU64,
    queue_wait_max_ns: AtomicU64,
    sign_ns: AtomicU64,
    http_ns: AtomicU64,
    authorized_to_http_ns: AtomicU64,
    attempts: AtomicU64,
    key_lookups: AtomicU64,
}
#[derive(Debug, serde::Serialize)]
pub struct RelayMetricsSnapshot {
    pub db_phase_ns: u64,
    pub db_slot_wait_ns: u64,
    pub db_calls: u64,
    pub connection_wait_ns: u64,
    pub lock_wait_ns: u64,
    pub scheduler_lock_wait_ns: u64,
    pub queued_jobs: u64,
    pub queue_wait_ns: u64,
    pub queue_wait_max_ns: u64,
    pub sign_ns: u64,
    pub http_ns: u64,
    pub authorized_to_http_ns: u64,
    pub attempts: u64,
    pub key_lookups: u64,
}
impl RelayMetrics {
    pub fn snapshot(&self) -> RelayMetricsSnapshot {
        RelayMetricsSnapshot {
            db_phase_ns: self.db_phase_ns.load(Ordering::Relaxed),
            db_slot_wait_ns: self.db_slot_wait_ns.load(Ordering::Relaxed),
            db_calls: self.db_calls.load(Ordering::Relaxed),
            connection_wait_ns: self.connection_wait_ns.load(Ordering::Relaxed),
            lock_wait_ns: self.lock_wait_ns.load(Ordering::Relaxed),
            scheduler_lock_wait_ns: self.scheduler_lock_wait_ns.load(Ordering::Relaxed),
            queued_jobs: self.queued_jobs.load(Ordering::Relaxed),
            queue_wait_ns: self.queue_wait_ns.load(Ordering::Relaxed),
            queue_wait_max_ns: self.queue_wait_max_ns.load(Ordering::Relaxed),
            sign_ns: self.sign_ns.load(Ordering::Relaxed),
            http_ns: self.http_ns.load(Ordering::Relaxed),
            authorized_to_http_ns: self.authorized_to_http_ns.load(Ordering::Relaxed),
            attempts: self.attempts.load(Ordering::Relaxed),
            key_lookups: self.key_lookups.load(Ordering::Relaxed),
        }
    }
}
type DbConnection<D> =
    <<D as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection;
struct DbPhase<'a> {
    _permit: SemaphorePermit<'a>,
    started: Instant,
    metrics: &'a RelayMetrics,
}
impl Drop for DbPhase<'_> {
    fn drop(&mut self) {
        self.metrics
            .db_phase_ns
            .fetch_add(self.started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.metrics.db_calls.fetch_add(1, Ordering::Relaxed);
    }
}

/// Deterministic stop points exist only in test builds, never configurable in production.
#[cfg(any(test, feature = "test-mode"))]
#[derive(Default)]
pub struct DeliveryPause {
    pub point: u8,
    pub reached: tokio::sync::Notify,
    pub resume: tokio::sync::Notify,
}
#[cfg(any(test, feature = "test-mode"))]
impl DeliveryPause {
    async fn at(&self, point: u8) {
        if self.point == point {
            self.reached.notify_one();
            self.resume.notified().await;
        }
    }
}

pub struct RelayDelivery<D> {
    #[cfg(any(test, feature = "test-mode"))]
    pause: Option<Arc<DeliveryPause>>,
    deps: Arc<D>,
    db_slots: Semaphore,
    pub metrics: Arc<RelayMetrics>,
}
impl<D> RelayDelivery<D>
where
    D: DependOnPostRelayStore
        + DependOnTransactionManager
        + DependOnPostRelayTransport
        + UnlockInstanceSigningKeyUseCase
        + 'static,
{
    pub fn new(deps: Arc<D>) -> Self {
        Self {
            deps,
            db_slots: Semaphore::new(4),
            metrics: Arc::new(RelayMetrics::default()),
            #[cfg(any(test, feature = "test-mode"))]
            pause: None,
        }
    }
    async fn db_slot(&self) -> DbPhase<'_> {
        let start = Instant::now();
        let permit = self.db_slots.acquire().await.expect("not closed");
        self.metrics
            .db_slot_wait_ns
            .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        DbPhase {
            _permit: permit,
            started: Instant::now(),
            metrics: &self.metrics,
        }
    }
    fn transaction<'a, F, T>(
        &'a self,
        operation: F,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = error_stack::Result<T, KernelError>> + Send + 'a>,
    >
    where
        F: for<'c> FnOnce(
                &'c mut DbConnection<D>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = error_stack::Result<T, KernelError>>
                        + Send
                        + 'c,
                >,
            > + Send
            + 'static,
        T: Send + 'static,
    {
        let metrics = self.metrics.clone();
        self.deps.transaction_manager().transaction(move |c| {
            Box::pin(async move {
                let result = operation(c).await;
                let timings = c.timings();
                metrics
                    .connection_wait_ns
                    .fetch_add(timings.connection_wait_ns, Ordering::Relaxed);
                metrics
                    .lock_wait_ns
                    .fetch_add(timings.lock_wait_ns, Ordering::Relaxed);
                metrics
                    .scheduler_lock_wait_ns
                    .fetch_add(timings.scheduler_lock_wait_ns, Ordering::Relaxed);
                result
            })
        })
    }
    #[cfg(any(test, feature = "test-mode"))]
    pub fn with_pause(mut self, pause: Arc<DeliveryPause>) -> Self {
        self.pause = Some(pause);
        self
    }
    pub async fn claim(&self, limit: u16) -> error_stack::Result<Vec<RelayLease>, KernelError> {
        self.claim_operation(limit, None).await
    }
    pub async fn claim_operation(
        &self,
        limit: u16,
        operation: Option<String>,
    ) -> error_stack::Result<Vec<RelayLease>, KernelError> {
        let _slot = self.db_slot().await;
        let d = self.deps.clone();
        let leases = self
            .transaction(move |c| {
                Box::pin(async move {
                    d.post_relay_store()
                        .claim_scheduled(c, limit, operation.as_deref())
                        .await
                })
            })
            .await?;
        for lease in &leases {
            self.metrics.queued_jobs.fetch_add(1, Ordering::Relaxed);
            self.metrics
                .queue_wait_ns
                .fetch_add(lease.queue_wait_ns, Ordering::Relaxed);
            self.metrics
                .queue_wait_max_ns
                .fetch_max(lease.queue_wait_ns, Ordering::Relaxed);
        }
        Ok(leases)
    }
    pub async fn maintain(&self) -> error_stack::Result<(), KernelError> {
        let _slot = self.db_slot().await;
        let d = self.deps.clone();
        let profiles = self
            .transaction(move |c| {
                Box::pin(async move { d.post_relay_store().maintenance_due(c, 16).await })
            })
            .await?;
        // Bounded one Profile per transaction. Called even when the sending pool is full.
        for p in profiles {
            let d = self.deps.clone();
            self.transaction(move |c| {
                Box::pin(async move {
                    d.post_relay_store().expire_due(c, &p, 128).await?;
                    d.post_relay_store().purge_expired(c, &p, 128).await?;
                    Ok(())
                })
            })
            .await?;
        }
        Ok(())
    }
    async fn record(
        &self,
        l: &RelayLease,
        r: AttemptResult,
        o: Option<AttemptObservation>,
    ) -> error_stack::Result<Recorded, KernelError> {
        let _slot = self.db_slot().await;
        let d = self.deps.clone();
        let l = l.clone();
        let random = rand::random();
        self.transaction(move |c| {
            Box::pin(async move {
                d.post_relay_store()
                    .record_attempt(c, &l, r, o.as_ref(), random)
                    .await
            })
        })
        .await
    }
    pub async fn deliver(&self, lease: RelayLease) -> error_stack::Result<Recorded, KernelError> {
        #[cfg(any(test, feature = "test-mode"))]
        if let Some(pause) = &self.pause {
            pause.at(0).await;
        }
        let d = &self.deps;
        let snapshot = {
            let _slot = self.db_slot().await;
            let d = self.deps.clone();
            let l = lease.clone();
            self.transaction(move |c| {
                Box::pin(async move { d.post_relay_store().prepare_send(c, &l).await })
            })
            .await?
        };
        let Some(snapshot) = snapshot else {
            return Ok(Recorded::Stale);
        };
        let valid = reqwest::Url::parse(&snapshot.recipient.origin).is_ok_and(|u| {
            let scheme = u.scheme() == "https"
                || (cfg!(any(test, feature = "test-mode")) && u.scheme() == "http");
            scheme
                && u.host_str().is_some()
                && u.username().is_empty()
                && u.password().is_none()
                && u.path() == "/"
                && u.query().is_none()
                && u.fragment().is_none()
        });
        if !valid {
            return self
                .record(&lease, AttemptResult::Permanent { status: None }, None)
                .await;
        }
        let allowed = d
            .post_relay_transport()
            .origin_allowed(&snapshot.recipient.origin);
        if !allowed {
            let _slot = self.db_slot().await;
            let d = self.deps.clone();
            return self
                .transaction(move |c| {
                    Box::pin(async move { d.post_relay_store().hold(c, &lease).await })
                })
                .await;
        }
        // Unlock drops its connection before KDF/parse. RSA/digest run on the blocking pool,
        // before authorization; runtime scheduling/signing never extends the committed budget.
        self.metrics.key_lookups.fetch_add(1, Ordering::Relaxed);
        let key_result = {
            let _slot = self.db_slot().await;
            d.unlock_instance_signing_key().await
        };
        let key = match key_result {
            Ok(k) => k,
            Err(_) => {
                return self
                    .record(
                        &lease,
                        AttemptResult::Retry {
                            status: None,
                            retry_after_secs: None,
                        },
                        None,
                    )
                    .await
            }
        };
        let origin = snapshot.recipient.origin.clone();
        let body = snapshot.envelope;
        let sign_started = Instant::now();
        let signature = tokio::task::spawn_blocking(move || {
            signed_headers(&origin, &body, key.key_id_uri(), |data| key.sign(data))
        })
        .await;
        self.metrics
            .sign_ns
            .fetch_add(sign_started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let headers = match signature {
            Ok(Ok(h)) => h,
            _ => {
                return self
                    .record(
                        &lease,
                        AttemptResult::Retry {
                            status: None,
                            retry_after_secs: None,
                        },
                        None,
                    )
                    .await
            }
        };
        // Start BEFORE the DB call: includes pool/lock/permission/commit latency conservatively.
        // DB authorized_at is later than this marker, so this mapping can only shorten, never
        // enlarge the deadline. A wall-clock jump cannot label a late response as in time.
        let start = Instant::now();
        #[cfg(any(test, feature = "test-mode"))]
        let pause = self.pause.clone();
        let authorization = {
            let _slot = self.db_slot().await;
            let d = self.deps.clone();
            self.transaction(move |c| {
                Box::pin(async move {
                    let authorization =
                        d.post_relay_store().authorize_send(c, &lease, true).await?;
                    #[cfg(any(test, feature = "test-mode"))]
                    if let Some(pause) = pause {
                        pause.at(1).await;
                    }
                    Ok(authorization)
                })
            })
            .await?
        }; // successful return means COMMIT acknowledged; uncertain commit returns Err and NO send.
        let permit = match authorization {
            Authorization::Permitted(p) => p,
            _ => return Ok(Recorded::Stale),
        };
        #[cfg(any(test, feature = "test-mode"))]
        if let Some(pause) = &self.pause {
            pause.at(2).await;
        }
        let remaining = permit.send_deadline - permit.authorized_at;
        let budget = if remaining.is_positive() {
            remaining.unsigned_abs()
        } else {
            std::time::Duration::ZERO
        };
        let deadline = start + budget;
        if Instant::now() >= deadline {
            return self
                .record(
                    &permit.lease,
                    AttemptResult::Retry {
                        status: None,
                        retry_after_secs: None,
                    },
                    None,
                )
                .await;
        }
        let http_started = Instant::now();
        self.metrics.authorized_to_http_ns.fetch_add(
            http_started.duration_since(start).as_nanos() as u64,
            Ordering::Relaxed,
        );
        let result = d
            .post_relay_transport()
            .send(&RelayHttpRequest {
                origin: &permit.recipient.origin,
                envelope: &permit.envelope,
                headers: &headers,
                deadline,
            })
            .await;
        let observed = Instant::now();
        self.metrics.http_ns.fetch_add(
            observed.duration_since(http_started).as_nanos() as u64,
            Ordering::Relaxed,
        );
        self.metrics.attempts.fetch_add(1, Ordering::Relaxed);
        #[cfg(any(test, feature = "test-mode"))]
        if let Some(pause) = &self.pause {
            pause.at(4).await;
        }
        tracing::debug!(
            http_ms = http_started.elapsed().as_millis() as u64,
            authorization_ms = http_started.duration_since(start).as_millis() as u64,
            "post relay attempt observed"
        );
        // Check again AFTER the future returns: timeout/ready races and runtime suspension are
        // not trusted in-deadline observations. Never clamp an after-deadline result into range.
        let (result, observation) = if observed >= deadline {
            (
                AttemptResult::Retry {
                    status: None,
                    retry_after_secs: None,
                },
                None,
            )
        } else {
            let result = match result {
                RelayHttpResult::Response {
                    status,
                    retry_after_secs,
                } => AttemptResult::http(status, retry_after_secs),
                RelayHttpResult::InvalidTarget => AttemptResult::Permanent { status: None },
                RelayHttpResult::Transient => AttemptResult::Retry {
                    status: None,
                    retry_after_secs: None,
                },
            };
            let observation = AttemptObservation {
                attempt_id: permit.attempt_id,
                observed_at: permit.authorized_at
                    + time::Duration::try_from(observed.duration_since(start))
                        .expect("ten seconds"),
            };
            (result, Some(observation))
        };
        self.record(&permit.lease, result, observation).await
    }
}
/// Contract Cavage signing input; never reserialize or otherwise alter the stored bytes.
pub fn signed_headers(
    origin: &str,
    body: &[u8],
    key_id: &str,
    sign: impl FnOnce(&[u8]) -> error_stack::Result<Vec<u8>, KernelError>,
) -> error_stack::Result<Vec<(String, String)>, KernelError> {
    signed_headers_at(
        origin,
        body,
        key_id,
        &httpdate::fmt_http_date(std::time::SystemTime::now()),
        sign,
    )
}
pub(crate) fn signed_headers_at(
    origin: &str,
    body: &[u8],
    key_id: &str,
    date: &str,
    sign: impl FnOnce(&[u8]) -> error_stack::Result<Vec<u8>, KernelError>,
) -> error_stack::Result<Vec<(String, String)>, KernelError> {
    let url = reqwest::Url::parse(origin)
        .map_err(|_| error_stack::Report::new(KernelError::Validation))?;
    let host = url[url::Position::BeforeHost..url::Position::AfterPort].to_string();
    let digest = format!("SHA-256={}", STANDARD.encode(Sha256::digest(body)));
    let input=format!("(request-target): post /internal/v1/emumet/activities\nhost: {host}\ndate: {date}\ndigest: {digest}");
    let sig = STANDARD.encode(sign(input.as_bytes())?);
    Ok(vec![("host".into(),host),("date".into(),date.into()),("digest".into(),digest),("content-type".into(),"application/json".into()),("signature".into(),format!("keyId=\"{key_id}\",algorithm=\"rsa-sha256\",headers=\"(request-target) host date digest\",signature=\"{sig}\""))])
}
