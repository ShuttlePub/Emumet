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
pub(crate) fn spawn(module: PostRelayModule) -> (watch::Sender<bool>, tokio::task::JoinHandle<()>) {
    let (stop, mut stopped) = watch::channel(false);
    let worker = Arc::new(RelayDelivery::new(Arc::new(module)));
    let handle = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        let mut poll = tokio::time::interval(Duration::from_millis(20));
        let mut maintenance = tokio::time::interval(Duration::from_secs(1));
        let mut telemetry = tokio::time::interval(Duration::from_secs(60));
        loop {
            tokio::select! {
                biased;
                _=stopped.changed()=>break,
                _=maintenance.tick()=>{if worker.maintain().await.is_err(){tracing::warn!("post relay maintenance failed");}},
                Some(result)=tasks.join_next(),if !tasks.is_empty()=>{if !matches!(result,Ok(Ok(_))) {tracing::warn!("post relay attempt failed; fenced lease recovery pending");}},
                _=telemetry.tick()=>tracing::info!(timings=?worker.metrics.snapshot(),"post relay cumulative timings"),
                _=poll.tick(),if tasks.len()<16=>{
                    match worker.claim((16-tasks.len()) as u16).await {
                        Ok(leases)=>for lease in leases {let worker=worker.clone(); tasks.spawn(async move {worker.deliver(lease).await});},
                        Err(_)=>tracing::warn!("post relay claim failed"),
                    }
                }
            }
        }
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
