//! Minimal relay DI/worker. Operator commands instantiate this, never AppModule/Ory/S3.
use application::instance_identity::{DependOnInstanceKeyCache, InstanceKeyCache};
use application::service::post_relay::delivery::RelayDelivery;
use driver::crypto::{
    Argon2Encryptor, FilePasswordProvider, Rsa2048PreparedSignerFactory, Rsa2048Verifier,
};
use driver::database::PostgresDatabase;
use driver::post_relay_delivery::HttpPostRelayTransport;
use kernel::interfaces::config::{DependOnPublicBaseUrl, PublicBaseUrl};
use kernel::interfaces::crypto::{
    DependOnKeyEncryptor, DependOnPasswordProvider, DependOnPreparedSignerFactory,
    DependOnSignatureVerifier,
};
use kernel::interfaces::post_relay::DependOnPostRelayTransport;
use kernel::KernelError;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

#[derive(Clone)]
pub(crate) struct PostRelayModule {
    pub(crate) db: PostgresDatabase,
    pub(crate) encryptor: Argon2Encryptor,
    pub(crate) cache: Arc<InstanceKeyCache>,
    pub(crate) password: FilePasswordProvider,
    pub(crate) origin: PublicBaseUrl,
    pub(crate) transport: HttpPostRelayTransport,
}
kernel::impl_database_delegation!(PostRelayModule, db, PostgresDatabase);
impl DependOnInstanceKeyCache for PostRelayModule {
    fn instance_key_cache(&self) -> &InstanceKeyCache {
        &self.cache
    }
}
impl DependOnPublicBaseUrl for PostRelayModule {
    fn public_base_url(&self) -> &PublicBaseUrl {
        &self.origin
    }
}
impl DependOnPasswordProvider for PostRelayModule {
    type PasswordProvider = FilePasswordProvider;
    fn password_provider(&self) -> &FilePasswordProvider {
        &self.password
    }
}
impl DependOnKeyEncryptor for PostRelayModule {
    type KeyEncryptor = Argon2Encryptor;
    fn key_encryptor(&self) -> &Argon2Encryptor {
        &self.encryptor
    }
}
impl DependOnPreparedSignerFactory for PostRelayModule {
    type PreparedSignerFactory = Rsa2048PreparedSignerFactory;
    fn prepared_signer_factory(&self) -> &Rsa2048PreparedSignerFactory {
        &Rsa2048PreparedSignerFactory
    }
}
impl DependOnSignatureVerifier for PostRelayModule {
    type SignatureVerifier = Rsa2048Verifier;
    fn signature_verifier(&self) -> &Rsa2048Verifier {
        &Rsa2048Verifier
    }
}
impl DependOnPostRelayTransport for PostRelayModule {
    type PostRelayTransport = HttpPostRelayTransport;
    fn post_relay_transport(&self) -> &HttpPostRelayTransport {
        &self.transport
    }
}

pub(crate) fn enabled() -> error_stack::Result<bool, KernelError> {
    match dotenvy::var("POST_RELAY_SENDER_ENABLED").as_deref() {
        Ok("true") | Err(dotenvy::Error::EnvVar(std::env::VarError::NotPresent)) => Ok(true),
        Ok("false") => Ok(false),
        _ => Err(error_stack::Report::new(KernelError::Validation)
            .attach_printable("POST_RELAY_SENDER_ENABLED must be true or false")),
    }
}
/// One bounded process pool, shared DB slots enforce 16 global/4 origin across all processes.
/// Maintenance gets a turn every second independently of in-flight HTTP.
pub(crate) fn spawn(
    module: PostRelayModule,
    sender_enabled: bool,
) -> (watch::Sender<bool>, tokio::task::JoinHandle<()>) {
    let (stop, mut stopped) = watch::channel(false);
    let worker = Arc::new(RelayDelivery::new(Arc::new(module)));
    let handle = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        let mut claims = tokio::task::JoinSet::new();
        let mut poll = tokio::time::interval(Duration::from_millis(20));
        let mut maintenance = tokio::time::interval(Duration::from_secs(1));
        let mut telemetry = tokio::time::interval(Duration::from_secs(60));
        loop {
            tokio::select! {
                biased;
                _=stopped.changed()=>break,
                _=maintenance.tick()=>{if worker.maintain().await.is_err(){tracing::warn!("post relay maintenance failed");}},
                Some(result)=tasks.join_next(),if !tasks.is_empty()=>{if !matches!(result,Ok(Ok(_))) {tracing::warn!("post relay attempt failed; fenced lease recovery pending");}},
                Some(result)=claims.join_next(),if !claims.is_empty()=>{
                    match result {
                        Ok(Ok(leases))=>for lease in leases {let worker=worker.clone(); tasks.spawn(async move {worker.deliver(lease).await});},
                        _=>tracing::warn!("post relay claim failed"),
                    }
                },
                _=telemetry.tick()=>tracing::info!(timings=?worker.metrics.snapshot(),"post relay cumulative timings"),
                _=poll.tick(),if sender_enabled && tasks.len()<16 && claims.is_empty()=>{
                    let worker=worker.clone();
                    let limit=(16-tasks.len()) as u16;
                    claims.spawn(async move {worker.claim(limit).await});
                }
            }
        }
        // Dropping the transaction on cancellation queues rollback; do not await its lock.
        claims.abort_all();
        while claims.join_next().await.is_some() {}
        // Stop claiming then bounded drain. Never reuses a permit on restart.
        let drain = async { while tasks.join_next().await.is_some() {} };
        if tokio::time::timeout(Duration::from_secs(12), drain)
            .await
            .is_err()
        {
            tasks.abort_all();
        }
    });
    (stop, handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn module(d: &driver::database::isolated_db::IsolatedDatabase) -> PostRelayModule {
        PostRelayModule {
            db: PostgresDatabase::from_pool(d.pool.clone()),
            encryptor: Argon2Encryptor::default(),
            cache: Arc::new(InstanceKeyCache::default()),
            password: FilePasswordProvider::new(),
            origin: PublicBaseUrl::new("https://emumet.example".into()),
            transport: HttpPostRelayTransport::new(
                driver::home_origin::HomeOriginAllowlist::from_json("[]").unwrap(),
            ),
        }
    }

    #[tokio::test]
    #[test_with::env(DATABASE_URL)]
    async fn stalled_claim_does_not_block_maintenance_or_shutdown() {
        let d = driver::database::isolated_db::IsolatedDatabase::create().await;
        let mut holder = d.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE post_relay_schedule IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *holder)
            .await
            .unwrap();
        let (stop, handle) = spawn(module(&d), true);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'LOCK TABLE post_relay_schedule%')")
                    .fetch_one(&d.pool).await.unwrap();
                if waiting { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        sqlx::query("INSERT INTO post_relay_profile_quota(profile_id,owner_account_id,delivery_state,delivery_rows,delivery_bytes,touched_at) VALUES(456,123,true,1,512,clock_timestamp()-interval '31 days')").execute(&d.pool).await.unwrap();
        sqlx::query("UPDATE post_relay_quota SET delivery_rows=delivery_rows+1,delivery_bytes=delivery_bytes+512 WHERE singleton").execute(&d.pool).await.unwrap();
        let maintained = tokio::time::timeout(Duration::from_millis(1500), async {
            loop {
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM post_relay_profile_quota WHERE profile_id=456)",
                )
                .fetch_one(&d.pool)
                .await
                .unwrap();
                if !exists {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        let _ = stop.send(true);
        let stopped = tokio::time::timeout(Duration::from_millis(500), handle).await;
        holder.rollback().await.unwrap();
        assert!(
            maintained.is_ok(),
            "maintenance must run during the 2s scheduler lock wait"
        );
        assert!(
            stopped.is_ok(),
            "shutdown must not await claim's lock timeout"
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND state='idle in transaction')")
                    .fetch_one(&d.pool).await.unwrap();
                if !waiting { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
    }
    #[tokio::test]
    #[test_with::env(DATABASE_URL)]
    async fn disabled_sender_still_collects_expired_empty_quota() {
        let d = driver::database::isolated_db::IsolatedDatabase::create().await;
        // Retained quota outlives deleted Accounts/Profiles; use a historical empty owner slot.
        sqlx::query("INSERT INTO post_relay_profile_quota(profile_id,owner_account_id,delivery_state,delivery_rows,delivery_bytes,touched_at) VALUES(456,123,true,1,512,clock_timestamp()-interval '31 days')").execute(&d.pool).await.unwrap();
        sqlx::query("UPDATE post_relay_quota SET delivery_rows=delivery_rows+1,delivery_bytes=delivery_bytes+512 WHERE singleton").execute(&d.pool).await.unwrap();
        let (stop, handle) = spawn(module(&d), false);
        let collected = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM post_relay_profile_quota WHERE profile_id=456)",
                )
                .fetch_one(&d.pool)
                .await
                .unwrap();
                if !exists {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        let _ = stop.send(true);
        handle.await.unwrap();
        assert!(
            collected.is_ok(),
            "sender stop must not suspend retention collection"
        );
    }
}
