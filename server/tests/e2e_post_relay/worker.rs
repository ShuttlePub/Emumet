//! An in-test relay worker (the same RelayDelivery as the server's) with the test-mode stop points
//! P0-P4, running against the SAME database as the spawned server. Used while the server's own
//! sender is switched off, so the stop points are deterministic and the revokers are the real
//! server API / projector.
use super::stack::PUBLIC;
use application::instance_identity::{DependOnInstanceKeyCache, InstanceKeyCache};
use application::service::post_relay::delivery::{DeliveryPause, RelayDelivery};
use driver::crypto::*;
use driver::database::PostgresDatabase;
use driver::home_origin::HomeOriginAllowlist;
use driver::post_relay_delivery::HttpPostRelayTransport;
use kernel::interfaces::{
    config::{DependOnPublicBaseUrl, PublicBaseUrl},
    crypto::*,
    database::*,
    post_relay::*,
};
use kernel::prelude::entity::ProfileId;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct TestModule {
    pub db: PostgresDatabase,
    password: FilePasswordProvider,
    cache: Arc<InstanceKeyCache>,
    encryptor: Argon2Encryptor,
    origin: PublicBaseUrl,
    transport: HttpPostRelayTransport,
}
kernel::impl_database_delegation!(TestModule, db, PostgresDatabase);
impl DependOnPasswordProvider for TestModule {
    type PasswordProvider = FilePasswordProvider;
    fn password_provider(&self) -> &FilePasswordProvider {
        &self.password
    }
}
impl DependOnKeyEncryptor for TestModule {
    type KeyEncryptor = Argon2Encryptor;
    fn key_encryptor(&self) -> &Argon2Encryptor {
        &self.encryptor
    }
}
impl DependOnPreparedSignerFactory for TestModule {
    type PreparedSignerFactory = Rsa2048PreparedSignerFactory;
    fn prepared_signer_factory(&self) -> &Rsa2048PreparedSignerFactory {
        &Rsa2048PreparedSignerFactory
    }
}
impl DependOnSignatureVerifier for TestModule {
    type SignatureVerifier = Rsa2048Verifier;
    fn signature_verifier(&self) -> &Rsa2048Verifier {
        &Rsa2048Verifier
    }
}
impl DependOnPublicBaseUrl for TestModule {
    fn public_base_url(&self) -> &PublicBaseUrl {
        &self.origin
    }
}
impl DependOnInstanceKeyCache for TestModule {
    fn instance_key_cache(&self) -> &InstanceKeyCache {
        &self.cache
    }
}
impl DependOnPostRelayTransport for TestModule {
    type PostRelayTransport = HttpPostRelayTransport;
    fn post_relay_transport(&self) -> &HttpPostRelayTransport {
        &self.transport
    }
}

impl TestModule {
    /// `server_dir` holds the master-key-password the spawned server provisioned the instance
    /// key with (FilePasswordProvider reads it from the current directory).
    pub fn new(db: PostgresDatabase, server_dir: &std::path::Path, receiver_origin: &str) -> Self {
        std::env::set_current_dir(server_dir).unwrap();
        Self {
            db,
            password: FilePasswordProvider::new(),
            cache: Arc::new(InstanceKeyCache::new(2, Duration::from_secs(900))),
            encryptor: Argon2Encryptor::default(),
            origin: PublicBaseUrl::new(PUBLIC.to_string()),
            transport: HttpPostRelayTransport::new(HomeOriginAllowlist::from_json("[]").unwrap()).with_test_origins(vec![receiver_origin.to_string()]),
        }
    }
    pub fn worker(&self) -> Arc<RelayDelivery<TestModule>> {
        Arc::new(RelayDelivery::new(Arc::new(self.clone())))
    }
    pub fn paused_worker(&self, point: u8) -> (Arc<RelayDelivery<TestModule>>, Arc<DeliveryPause>) {
        let pause = Arc::new(DeliveryPause { point, ..Default::default() });
        (Arc::new(RelayDelivery::new(Arc::new(self.clone())).with_pause(pause.clone())), pause)
    }
    /// Operator path: quarantine the receipt (permanent failure) and stage a manual redelivery
    /// exactly like `server post-relay redeliver` does through the store.
    pub async fn stage_manual(&self, profile_id: i64, receipt: &str, operation: &str) {
        let db = &self.db;
        let mut t = db.get_transaction().await.unwrap();
        let lease = db.post_relay_store().claim_due(t.connection(), 1).await.unwrap().remove(0);
        t.commit().await.unwrap();
        assert_eq!(lease.receipt, receipt);
        let mut t = db.get_transaction().await.unwrap();
        db.post_relay_store().record_attempt(t.connection(), &lease, AttemptResult::Permanent { status: Some(403) }, None, 0).await.unwrap();
        t.commit().await.unwrap();
        let request = OperatorRequest {
            id: operation.into(),
            profile_id: ProfileId::new(profile_id),
            receipt: Some(receipt.into()),
            action: OperatorAction::Redeliver,
            operator: "t1-e2e".into(),
            reason: "race test".into(),
        };
        let mut t = db.get_transaction().await.unwrap();
        db.post_relay_store().begin_operator(t.connection(), &request).await.unwrap();
        db.post_relay_store().operator_redeliver(t.connection(), &request.id).await.unwrap();
        t.commit().await.unwrap();
    }
}
