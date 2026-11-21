use super::*;
use driver::crypto::{
    Argon2Encryptor, FilePasswordProvider, Rsa2048PreparedSignerFactory, Rsa2048RawGenerator,
    Rsa2048Verifier,
};
use driver::database::isolated_db::IsolatedDatabase;
use driver::database::PostgresDatabase;
use kernel::interfaces::config::PublicBaseUrl;
use kernel::interfaces::crypto::{
    DependOnRawKeyGenerator, EncryptedPrivateKey, PreparedSigner, RawKeyGenerator,
};
use kernel::interfaces::database::DependOnDatabaseConnection;
use sqlx::Executor;
use std::sync::atomic::{AtomicUsize, Ordering};
use zeroize::Zeroizing;

const BASE: &str = "https://emumet.example";

struct CountingEncryptor {
    inner: Argon2Encryptor,
    decrypts: Arc<AtomicUsize>,
}

impl KeyEncryptor for CountingEncryptor {
    fn encrypt(
        &self,
        private_key_pem: &[u8],
        password: &[u8],
        algorithm: SigningAlgorithm,
    ) -> error_stack::Result<EncryptedPrivateKey, KernelError> {
        self.inner.encrypt(private_key_pem, password, algorithm)
    }

    fn decrypt(
        &self,
        encrypted: &EncryptedPrivateKey,
        password: &[u8],
    ) -> error_stack::Result<Zeroizing<Vec<u8>>, KernelError> {
        self.decrypts.fetch_add(1, Ordering::SeqCst);
        self.inner.decrypt(encrypted, password)
    }
}

/// Counts how often a PEM is parsed into a signer (`prepares`) and how often a parsed signer signs
/// (`signs`).
struct CountingSignerFactory {
    inner: Rsa2048PreparedSignerFactory,
    prepares: Arc<AtomicUsize>,
    signs: Arc<AtomicUsize>,
}

struct CountingSigner {
    inner: Box<dyn PreparedSigner>,
    signs: Arc<AtomicUsize>,
}

impl PreparedSignerFactory for CountingSignerFactory {
    fn prepare(
        &self,
        private_key_pem: &[u8],
        algorithm: SigningAlgorithm,
    ) -> error_stack::Result<Box<dyn PreparedSigner>, KernelError> {
        self.prepares.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(CountingSigner {
            inner: self.inner.prepare(private_key_pem, algorithm)?,
            signs: self.signs.clone(),
        }))
    }
}

impl PreparedSigner for CountingSigner {
    fn sign(&self, data: &[u8]) -> error_stack::Result<Vec<u8>, KernelError> {
        self.signs.fetch_add(1, Ordering::SeqCst);
        self.inner.sign(data)
    }

    fn algorithm(&self) -> SigningAlgorithm {
        self.inner.algorithm()
    }
}

/// One "process": its own cache, counters and password, on a shared database.
#[derive(Clone)]
struct TestModule {
    database: PostgresDatabase,
    password_provider: Arc<FilePasswordProvider>,
    raw_key_generator: Arc<Rsa2048RawGenerator>,
    key_encryptor: Arc<CountingEncryptor>,
    signer_factory: Arc<CountingSignerFactory>,
    verifier: Rsa2048Verifier,
    public_base_url: PublicBaseUrl,
    cache: Arc<InstanceKeyCache>,
}

kernel::impl_database_delegation!(TestModule, database, PostgresDatabase);

impl DependOnPasswordProvider for TestModule {
    type PasswordProvider = FilePasswordProvider;
    fn password_provider(&self) -> &Self::PasswordProvider {
        self.password_provider.as_ref()
    }
}

impl DependOnRawKeyGenerator for TestModule {
    type RawKeyGenerator = Rsa2048RawGenerator;
    fn raw_key_generator(&self) -> &Self::RawKeyGenerator {
        self.raw_key_generator.as_ref()
    }
}

impl DependOnKeyEncryptor for TestModule {
    type KeyEncryptor = CountingEncryptor;
    fn key_encryptor(&self) -> &Self::KeyEncryptor {
        self.key_encryptor.as_ref()
    }
}

impl DependOnPreparedSignerFactory for TestModule {
    type PreparedSignerFactory = CountingSignerFactory;
    fn prepared_signer_factory(&self) -> &Self::PreparedSignerFactory {
        self.signer_factory.as_ref()
    }
}

impl DependOnSignatureVerifier for TestModule {
    type SignatureVerifier = Rsa2048Verifier;
    fn signature_verifier(&self) -> &Self::SignatureVerifier {
        &self.verifier
    }
}

impl DependOnPublicBaseUrl for TestModule {
    fn public_base_url(&self) -> &PublicBaseUrl {
        &self.public_base_url
    }
}

impl DependOnInstanceKeyCache for TestModule {
    fn instance_key_cache(&self) -> &InstanceKeyCache {
        self.cache.as_ref()
    }
}

struct Env {
    iso: IsolatedDatabase,
    password: tempfile::NamedTempFile,
}

impl Env {
    async fn new() -> Self {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        Self::on(iso)
    }

    fn on(iso: IsolatedDatabase) -> Self {
        let password = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(password.path(), b"instance-identity-password").unwrap();
        Self { iso, password }
    }

    /// A fresh process (empty cache, own counters) with the given password file and origin.
    fn process_with(
        &self,
        password: &std::path::Path,
        base_url: &str,
    ) -> (TestModule, Arc<AtomicUsize>) {
        let decrypts = Arc::new(AtomicUsize::new(0));
        let module = TestModule {
            database: self.iso.database.clone(),
            password_provider: Arc::new(FilePasswordProvider::with_paths(password, password)),
            raw_key_generator: Arc::new(Rsa2048RawGenerator),
            key_encryptor: Arc::new(CountingEncryptor {
                inner: Argon2Encryptor::default(),
                decrypts: decrypts.clone(),
            }),
            signer_factory: Arc::new(CountingSignerFactory {
                inner: Rsa2048PreparedSignerFactory,
                prepares: Arc::new(AtomicUsize::new(0)),
                signs: Arc::new(AtomicUsize::new(0)),
            }),
            verifier: Rsa2048Verifier,
            public_base_url: PublicBaseUrl::new(base_url.to_string()),
            cache: Arc::new(InstanceKeyCache::default()),
        };
        (module, decrypts)
    }

    fn process(&self) -> (TestModule, Arc<AtomicUsize>) {
        self.process_with(self.password.path(), BASE)
    }

    async fn counts(&self) -> (i64, i64) {
        let identities: i64 = sqlx::query_scalar("SELECT count(*) FROM instance_identity")
            .fetch_one(&self.iso.pool)
            .await
            .unwrap();
        let keys: i64 = sqlx::query_scalar("SELECT count(*) FROM instance_signing_keys")
            .fetch_one(&self.iso.pool)
            .await
            .unwrap();
        (identities, keys)
    }

    async fn stored_key(&self) -> (String, String, serde_json::Value, String) {
        sqlx::query_as(
            "SELECT id, public_key_pem, encrypted_private_key, key_id_uri FROM instance_signing_keys",
        )
        .fetch_one(&self.iso.pool)
        .await
        .unwrap()
    }

    /// The immutability triggers are the production guard; tests that simulate a damaged database
    /// have to step around them.
    async fn damage(&self, sql: &str) {
        let mut connection = self.iso.pool.acquire().await.unwrap();
        connection
            .execute("ALTER TABLE instance_signing_keys DISABLE TRIGGER USER")
            .await
            .unwrap();
        connection.execute(sql).await.unwrap();
        connection
            .execute("ALTER TABLE instance_signing_keys ENABLE TRIGGER USER")
            .await
            .unwrap();
    }
}

fn problem(report: &Report<KernelError>) -> Option<&InstanceIdentityProblem> {
    report.downcast_ref::<InstanceIdentityProblem>()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_starts_create_one_identity_and_one_active_key() {
    let env = Env::new().await;
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let (module, _) = env.process();
        tasks.push(tokio::spawn(async move {
            module.ensure_instance_identity().await.unwrap()
        }));
    }
    let mut reports = Vec::new();
    for task in tasks {
        reports.push(task.await.unwrap());
    }
    assert_eq!(reports.iter().filter(|report| report.created).count(), 1);
    assert!(reports
        .iter()
        .all(|report| report.key_id_uri == reports[0].key_id_uri));
    assert_eq!(env.counts().await, (1, 1));
    let active: i64 =
        sqlx::query_scalar("SELECT count(*) FROM instance_signing_keys WHERE revoked_at IS NULL")
            .fetch_one(&env.iso.pool)
            .await
            .unwrap();
    assert_eq!(active, 1);
}

/// The advisory lock is the serialization; the unique constraints are the backstop. Without the
/// lock, concurrent creators must still end with exactly one identity and one active key (the
/// losers get `Concurrency`, nothing is replaced).
#[test_with::env(DATABASE_URL)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unique_constraints_alone_keep_one_identity_and_one_active_key() {
    let env = Env::new().await;
    let (module, _) = env.process();
    let origin = InstanceOrigin::parse(BASE).unwrap();
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let module = module.clone();
        let origin = origin.clone();
        tasks.push(tokio::spawn(async move {
            let repository = module.instance_identity_repository();
            let mut connection = module.database_connection().connection().await.unwrap();
            let now = time::OffsetDateTime::now_utc();
            let identity_created = repository
                .create_identity(&mut connection, &InstanceIdentity::new(origin.clone(), now))
                .await;
            let pair = Rsa2048RawGenerator.generate_raw().unwrap();
            let id = InstanceSigningKeyId::generate();
            let key = InstanceSigningKey::new(
                id.clone(),
                1,
                pair.algorithm,
                module
                    .key_encryptor()
                    .encrypt(&pair.private_key_pem, b"pw", pair.algorithm)
                    .unwrap(),
                pair.public_key_pem,
                origin.key_id_uri(&id),
                now,
                None,
                None,
            );
            let key_created = repository.create_signing_key(&mut connection, &key).await;
            (identity_created, key_created)
        }));
    }
    let mut identity_wins = 0;
    let mut key_wins = 0;
    for task in tasks {
        let (identity, key) = task.await.unwrap();
        match identity {
            Ok(()) => identity_wins += 1,
            Err(e) => assert_eq!(e.current_context(), &KernelError::Concurrency),
        }
        match key {
            Ok(()) => key_wins += 1,
            Err(e) => assert!(matches!(
                e.current_context(),
                KernelError::Concurrency | KernelError::Internal
            )),
        }
    }
    assert_eq!(identity_wins, 1);
    assert_eq!(key_wins, 1);
    assert_eq!(env.counts().await, (1, 1));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn restart_keeps_identity_and_key_unchanged() {
    let env = Env::new().await;
    let (first, _) = env.process();
    let created = first.ensure_instance_identity().await.unwrap();
    assert!(created.created);
    let before = env.stored_key().await;
    let actor_before = serde_json::to_value(first.get_instance_actor().await.unwrap()).unwrap();

    for _ in 0..2 {
        let (restarted, _) = env.process();
        let report = restarted.ensure_instance_identity().await.unwrap();
        assert!(!report.created);
        assert_eq!(report.key_id_uri, created.key_id_uri);
        let actor = serde_json::to_value(restarted.get_instance_actor().await.unwrap()).unwrap();
        assert_eq!(actor, actor_before);
    }
    assert_eq!(env.stored_key().await, before);
    assert_eq!(env.counts().await, (1, 1));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn actor_document_is_an_application_with_public_keys_only() {
    let env = Env::new().await;
    let (module, _) = env.process();
    let report = module.ensure_instance_identity().await.unwrap();
    let actor = module.get_instance_actor().await.unwrap();
    let json = serde_json::to_value(&actor).unwrap();

    assert_eq!(json["id"], "https://emumet.example/ap/instance");
    assert_eq!(json["type"], "Application");
    let keys = json["publicKey"].as_array().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["id"], report.key_id_uri);
    assert!(report
        .key_id_uri
        .starts_with("https://emumet.example/ap/instance#key-"));
    assert_eq!(keys[0]["owner"], "https://emumet.example/ap/instance");
    assert!(keys[0]["publicKeyPem"]
        .as_str()
        .unwrap()
        .starts_with("-----BEGIN PUBLIC KEY-----"));

    let text = json.to_string();
    let (_, _, encrypted, _) = env.stored_key().await;
    assert!(!text.contains("PRIVATE"));
    assert!(!text.contains(encrypted["ciphertext"].as_str().unwrap()));
    assert!(!text.contains(encrypted["salt"].as_str().unwrap()));
    assert!(!text.contains("encrypted"));
    assert!(!text.contains("ciphertext"));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_different_configured_origin_is_refused_and_nothing_is_regenerated() {
    let env = Env::new().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    let before = env.stored_key().await;

    for other in ["https://other.example", "http://emumet.example"] {
        let (other_origin, _) = env.process_with(env.password.path(), other);
        let error = other_origin.ensure_instance_identity().await.unwrap_err();
        assert_eq!(
            problem(&error),
            Some(&InstanceIdentityProblem::OriginMismatch)
        );
        let error = other_origin.get_instance_actor().await.unwrap_err();
        assert_eq!(
            problem(&error),
            Some(&InstanceIdentityProblem::OriginMismatch)
        );
        let error = other_origin
            .unlock_instance_signing_key()
            .await
            .unwrap_err();
        assert_eq!(
            problem(&error),
            Some(&InstanceIdentityProblem::OriginMismatch)
        );
    }
    assert_eq!(env.stored_key().await, before);
    assert_eq!(env.counts().await, (1, 1));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_equivalent_spelling_of_the_origin_is_not_a_mismatch() {
    let env = Env::new().await;
    let (module, _) = env.process();
    let created = module.ensure_instance_identity().await.unwrap();
    let (spelled, _) = env.process_with(env.password.path(), "https://Emumet.Example:443/");
    let report = spelled.ensure_instance_identity().await.unwrap();
    assert!(!report.created);
    assert_eq!(report.key_id_uri, created.key_id_uri);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_undecryptable_key_is_an_error_not_a_missing_key() {
    let env = Env::new().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    let before = env.stored_key().await;

    let wrong_password = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(wrong_password.path(), b"another-master-password").unwrap();
    let (wrong, _) = env.process_with(wrong_password.path(), BASE);
    let error = wrong.ensure_instance_identity().await.unwrap_err();
    assert_eq!(
        problem(&error),
        Some(&InstanceIdentityProblem::UndecryptableKey)
    );

    assert_eq!(env.stored_key().await, before);
    assert_eq!(env.counts().await, (1, 1));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_corrupted_private_key_is_an_error_not_a_missing_key() {
    let env = Env::new().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    env.damage(
        "UPDATE instance_signing_keys SET encrypted_private_key = jsonb_set(encrypted_private_key, '{ciphertext}', '\"AAAA\"')",
    )
    .await;
    let damaged = env.stored_key().await;

    let (restarted, _) = env.process();
    let error = restarted.ensure_instance_identity().await.unwrap_err();
    assert_eq!(
        problem(&error),
        Some(&InstanceIdentityProblem::UndecryptableKey)
    );
    assert_eq!(env.stored_key().await, damaged);
    assert_eq!(env.counts().await, (1, 1));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_unreadable_private_key_row_is_an_error_not_a_missing_key() {
    let env = Env::new().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    env.damage("UPDATE instance_signing_keys SET encrypted_private_key = '{\"unexpected\": true}'")
        .await;
    let damaged = env.stored_key().await;

    let (restarted, _) = env.process();
    let error = restarted.ensure_instance_identity().await.unwrap_err();
    assert_eq!(error.current_context(), &KernelError::Internal);
    assert_eq!(env.stored_key().await, damaged);
    assert_eq!(env.counts().await, (1, 1));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_private_key_that_does_not_match_the_public_key_is_refused() {
    let env = Env::new().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    let other = Rsa2048RawGenerator.generate_raw().unwrap().public_key_pem;
    env.damage(&format!(
        "UPDATE instance_signing_keys SET public_key_pem = '{other}'"
    ))
    .await;

    let (restarted, _) = env.process();
    let error = restarted.ensure_instance_identity().await.unwrap_err();
    assert_eq!(
        problem(&error),
        Some(&InstanceIdentityProblem::KeyPairMismatch)
    );
    assert_eq!(env.counts().await, (1, 1));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_damaged_key_id_is_refused() {
    let env = Env::new().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    env.damage(
        "UPDATE instance_signing_keys SET key_id_uri = 'https://emumet.example/ap/instance#key-0'",
    )
    .await;

    let (restarted, _) = env.process();
    let error = restarted.ensure_instance_identity().await.unwrap_err();
    assert_eq!(
        problem(&error),
        Some(&InstanceIdentityProblem::KeyIdUriMismatch)
    );
    let error = restarted.get_instance_actor().await.unwrap_err();
    assert_eq!(
        problem(&error),
        Some(&InstanceIdentityProblem::KeyIdUriMismatch)
    );
    assert_eq!(env.counts().await, (1, 1));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn keys_without_an_identity_are_not_repaired_by_creating_an_identity() {
    let env = Env::new().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    // A damaged database: the identity row is gone but its key remains.
    {
        let mut connection = env.iso.pool.acquire().await.unwrap();
        connection
            .execute("ALTER TABLE instance_signing_keys DISABLE TRIGGER USER")
            .await
            .unwrap();
        connection
            .execute("ALTER TABLE instance_signing_keys DROP CONSTRAINT instance_signing_keys_singleton_fkey")
            .await
            .unwrap();
        connection
            .execute("ALTER TABLE instance_identity DISABLE TRIGGER USER")
            .await
            .unwrap();
        connection
            .execute("DELETE FROM instance_identity")
            .await
            .unwrap();
    }
    let (restarted, _) = env.process();
    let error = restarted.ensure_instance_identity().await.unwrap_err();
    assert_eq!(
        problem(&error),
        Some(&InstanceIdentityProblem::KeysWithoutIdentity)
    );
    assert_eq!(env.counts().await, (0, 1));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_identity_without_an_active_key_is_not_repaired_by_generating_one() {
    let env = Env::new().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    // K2 would revoke through its own path; here the row is revoked directly.
    sqlx::query("UPDATE instance_signing_keys SET revoked_at = now()")
        .execute(&env.iso.pool)
        .await
        .unwrap();

    let (restarted, _) = env.process();
    let error = restarted.ensure_instance_identity().await.unwrap_err();
    assert_eq!(problem(&error), Some(&InstanceIdentityProblem::NoActiveKey));
    assert_eq!(env.counts().await, (1, 1));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_identity_whose_keys_are_all_retired_has_no_active_key_either() {
    let env = Env::new().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    sqlx::query("UPDATE instance_signing_keys SET retired_at = now()")
        .execute(&env.iso.pool)
        .await
        .unwrap();

    let (restarted, _) = env.process();
    let error = restarted.ensure_instance_identity().await.unwrap_err();
    assert_eq!(problem(&error), Some(&InstanceIdentityProblem::NoActiveKey));
    assert_eq!(env.counts().await, (1, 1));
}

/// A new key as K2's rotation would add it (K1 itself never adds a second key).
fn new_key(module: &TestModule, origin: &InstanceOrigin) -> InstanceSigningKey {
    let pair = Rsa2048RawGenerator.generate_raw().unwrap();
    let id = InstanceSigningKeyId::generate();
    let encrypted = module
        .key_encryptor()
        .encrypt(
            &pair.private_key_pem,
            b"instance-identity-password",
            pair.algorithm,
        )
        .unwrap();
    InstanceSigningKey::new(
        id.clone(),
        1,
        pair.algorithm,
        encrypted,
        pair.public_key_pem,
        origin.key_id_uri(&id),
        time::OffsetDateTime::now_utc(),
        None,
        None,
    )
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_second_active_key_is_impossible() {
    let env = Env::new().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    let origin = InstanceOrigin::parse(BASE).unwrap();
    let mut executor = module.database_connection().connection().await.unwrap();

    let rejected = module
        .instance_identity_repository()
        .create_signing_key(&mut executor, &new_key(&module, &origin))
        .await
        .unwrap_err();
    assert_eq!(rejected.current_context(), &KernelError::Concurrency);
    assert_eq!(env.counts().await, (1, 1));
}

/// Publication and signing are separate: with exactly one active key, the document still lists
/// every key that has not been revoked.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn the_document_publishes_every_unrevoked_key_while_exactly_one_key_is_active() {
    let env = Env::new().await;
    let (module, _) = env.process();
    let first = module.ensure_instance_identity().await.unwrap();
    let origin = InstanceOrigin::parse(BASE).unwrap();
    let repository = module.instance_identity_repository();

    // K2's rotation: the first key stops signing (retired) but stays published, then the new active
    // key is added.
    sqlx::query("UPDATE instance_signing_keys SET retired_at = now()")
        .execute(&env.iso.pool)
        .await
        .unwrap();
    let second = new_key(&module, &origin);
    let mut executor = module.database_connection().connection().await.unwrap();
    repository
        .create_signing_key(&mut executor, &second)
        .await
        .unwrap();
    drop(executor);
    assert_eq!(env.counts().await, (1, 2));

    // Two public keys are published, both owned by the instance actor; the keys are distinct.
    let actor = module.get_instance_actor().await.unwrap();
    let ids: Vec<&str> = actor.public_key.iter().map(|key| key.id.as_str()).collect();
    assert_eq!(ids, [first.key_id_uri.as_str(), second.key_id_uri.as_str()]);
    assert!(actor
        .public_key
        .iter()
        .all(|key| key.owner == origin.actor_id()));
    assert_ne!(
        actor.public_key[0].public_key_pem,
        actor.public_key[1].public_key_pem
    );

    // Exactly one key is active: the new one signs, and a third active key is rejected.
    let (restarted, _) = env.process();
    let report = restarted.ensure_instance_identity().await.unwrap();
    assert!(!report.created);
    assert_eq!(report.key_id_uri, second.key_id_uri);
    let unlocked = restarted.unlock_instance_signing_key().await.unwrap();
    assert_eq!(unlocked.key_id_uri(), second.key_id_uri);
    let signature = unlocked.sign(b"payload").unwrap();
    assert!(Rsa2048Verifier
        .verify(
            b"payload",
            &signature,
            actor.public_key[1].public_key_pem.as_bytes()
        )
        .unwrap());
    assert!(!Rsa2048Verifier
        .verify(
            b"payload",
            &signature,
            actor.public_key[0].public_key_pem.as_bytes()
        )
        .unwrap());
    let mut executor = module.database_connection().connection().await.unwrap();
    let rejected = repository
        .create_signing_key(&mut executor, &new_key(&module, &origin))
        .await
        .unwrap_err();
    assert_eq!(rejected.current_context(), &KernelError::Concurrency);
    drop(executor);
    assert_eq!(env.counts().await, (1, 2));

    // A revoked key is no longer published; the active key and the document stay valid.
    sqlx::query("UPDATE instance_signing_keys SET revoked_at = now() WHERE key_id_uri = $1")
        .bind(&first.key_id_uri)
        .execute(&env.iso.pool)
        .await
        .unwrap();
    let actor = module.get_instance_actor().await.unwrap();
    assert_eq!(actor.public_key.len(), 1);
    assert_eq!(actor.public_key[0].id, second.key_id_uri);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_key_id_is_never_reused() {
    let env = Env::new().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    let (id, _, _, uri) = env.stored_key().await;

    // Even a revoked key keeps its id: the same id / key id URI cannot be inserted again, and the
    // row cannot be deleted.
    sqlx::query("UPDATE instance_signing_keys SET revoked_at = now()")
        .execute(&env.iso.pool)
        .await
        .unwrap();
    let origin = InstanceOrigin::parse(BASE).unwrap();
    let reused = InstanceSigningKey::new(
        InstanceSigningKeyId::parse(&id).unwrap(),
        1,
        SigningAlgorithm::Rsa2048,
        module
            .key_encryptor()
            .encrypt(b"x", b"pw", SigningAlgorithm::Rsa2048)
            .unwrap(),
        "pem".to_string(),
        uri.clone(),
        time::OffsetDateTime::now_utc(),
        None,
        None,
    );
    assert_eq!(origin.key_id_uri(reused.id()), uri);
    let mut executor = module.database_connection().connection().await.unwrap();
    let error = module
        .instance_identity_repository()
        .create_signing_key(&mut executor, &reused)
        .await
        .unwrap_err();
    assert_eq!(error.current_context(), &KernelError::Concurrency);
    drop(executor);

    assert!(sqlx::query("DELETE FROM instance_signing_keys")
        .execute(&env.iso.pool)
        .await
        .is_err());
    assert!(sqlx::query("TRUNCATE instance_signing_keys")
        .execute(&env.iso.pool)
        .await
        .is_err());
    assert_eq!(env.counts().await, (1, 1));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn identity_and_key_material_are_immutable_in_the_database() {
    let env = Env::new().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    for statement in [
        "UPDATE instance_identity SET public_origin = 'https://other.example'",
        "DELETE FROM instance_identity",
        "TRUNCATE instance_identity CASCADE",
        "UPDATE instance_signing_keys SET public_key_pem = 'x'",
        "UPDATE instance_signing_keys SET encrypted_private_key = '{}'",
        "UPDATE instance_signing_keys SET key_id_uri = 'https://other.example/ap/instance#key-0'",
        "UPDATE instance_signing_keys SET id = repeat('0', 32)",
    ] {
        assert!(
            sqlx::query(statement).execute(&env.iso.pool).await.is_err(),
            "{statement}"
        );
    }
    // The state of a key (retirement, revocation, revision) is the only thing that may change (K2).
    sqlx::query(
        "UPDATE instance_signing_keys SET revision = revision + 1, retired_at = now(), revoked_at = now()",
    )
    .execute(&env.iso.pool)
    .await
    .unwrap();
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn the_kdf_does_not_run_per_use() {
    let env = Env::new().await;
    let (module, decrypts) = env.process();
    module.ensure_instance_identity().await.unwrap();
    let after_start = decrypts.load(Ordering::SeqCst);
    assert_eq!(after_start, 1, "start-up unlocks the key exactly once");

    let mut tasks = Vec::new();
    for _ in 0..40 {
        let module = module.clone();
        tasks.push(tokio::spawn(async move {
            module.unlock_instance_signing_key().await.unwrap()
        }));
    }
    for task in tasks {
        let key = task.await.unwrap();
        assert!(key
            .key_id_uri()
            .starts_with("https://emumet.example/ap/instance#key-"));
        assert!(!key.sign(b"payload").unwrap().is_empty());
    }
    assert_eq!(decrypts.load(Ordering::SeqCst), after_start);
}

/// The cache holds a parsed signer: however many times the key is used, the PEM was decrypted and
/// parsed once (at unlock), and every signature goes through the parsed handle.
#[test_with::env(DATABASE_URL)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cache_holds_a_parsed_signer_that_signs_without_parsing_again() {
    let env = Env::new().await;
    let (module, decrypts) = env.process();
    module.ensure_instance_identity().await.unwrap();
    let factory = module.signer_factory.clone();
    // Start-up: one decrypt, one parse, one self-check signature.
    assert_eq!(decrypts.load(Ordering::SeqCst), 1);
    assert_eq!(factory.prepares.load(Ordering::SeqCst), 1);
    assert_eq!(factory.signs.load(Ordering::SeqCst), 1);

    let public_key_pem = module.get_instance_actor().await.unwrap().public_key[0]
        .public_key_pem
        .clone();
    let mut tasks = Vec::new();
    for worker in 0..8 {
        let module = module.clone();
        tasks.push(tokio::spawn(async move {
            let mut signatures = Vec::new();
            for attempt in 0..25 {
                let key = module.unlock_instance_signing_key().await.unwrap();
                let data = format!("request {worker}-{attempt}").into_bytes();
                signatures.push((data.clone(), key.sign(&data).unwrap()));
            }
            signatures
        }));
    }
    let mut signed = 0;
    for task in tasks {
        for (data, signature) in task.await.unwrap() {
            assert!(Rsa2048Verifier
                .verify(&data, &signature, public_key_pem.as_bytes())
                .unwrap());
            signed += 1;
        }
    }
    assert_eq!(signed, 200);
    assert_eq!(decrypts.load(Ordering::SeqCst), 1, "no KDF per signature");
    assert_eq!(
        factory.prepares.load(Ordering::SeqCst),
        1,
        "no PEM parse per signature"
    );
    assert_eq!(factory.signs.load(Ordering::SeqCst), 201);
}

/// A key that decrypts but is not a usable private key is refused, like any other damage.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_decryptable_but_unparsable_private_key_is_refused() {
    let env = Env::new().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    let encrypted = module
        .key_encryptor()
        .encrypt(
            b"not a pem",
            b"instance-identity-password",
            SigningAlgorithm::Rsa2048,
        )
        .unwrap();
    let json = serde_json::to_string(&encrypted).unwrap();
    env.damage(&format!(
        "UPDATE instance_signing_keys SET encrypted_private_key = '{json}'::jsonb"
    ))
    .await;

    let (restarted, _) = env.process();
    let error = restarted.ensure_instance_identity().await.unwrap_err();
    assert_eq!(
        problem(&error),
        Some(&InstanceIdentityProblem::UnparsableKey)
    );
    assert_eq!(env.counts().await, (1, 1));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unlocked_key_signs_and_verifies_with_the_published_public_key() {
    let env = Env::new().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    let unlocked = module.unlock_instance_signing_key().await.unwrap();
    let actor = module.get_instance_actor().await.unwrap();
    let signature = unlocked.sign(b"payload").unwrap();
    assert!(Rsa2048Verifier
        .verify(
            b"payload",
            &signature,
            actor.public_key[0].public_key_pem.as_bytes()
        )
        .unwrap());
    assert!(!format!("{unlocked:?}").contains("PRIVATE"));
    assert_eq!(unlocked.key_id_uri(), actor.public_key[0].id);
}

/// An instance key is stored in its own tables with its own key id namespace: provisioning it
/// leaves the Profile key tables untouched.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn provisioning_does_not_touch_profile_keys() {
    let env = Env::new().await;
    let count_profile_keys = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM profile_signing_keys")
            .fetch_one(&env.iso.pool)
            .await
            .unwrap()
    };
    let before = count_profile_keys().await;
    let (module, _) = env.process();
    module.ensure_instance_identity().await.unwrap();
    assert_eq!(count_profile_keys().await, before);
}

/// Applying the migration to a database that already has the (production) schema and data of
/// every other migration is purely additive.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn migration_applies_to_a_database_that_already_has_the_current_schema() {
    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create_without_migration(20261122000100).await;
    let tables_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public' AND table_name LIKE 'instance_%'",
    )
    .fetch_one(&iso.pool)
    .await
    .unwrap();
    assert_eq!(tables_before, 0);
    let existing_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.columns WHERE table_schema = 'public' AND table_name NOT LIKE 'instance_%'",
    )
    .fetch_one(&iso.pool)
    .await
    .unwrap();

    iso.apply_reset().await;

    let existing_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.columns WHERE table_schema = 'public' AND table_name NOT LIKE 'instance_%'",
    )
    .fetch_one(&iso.pool)
    .await
    .unwrap();
    assert_eq!(existing_before, existing_after);
    let env = Env::on(iso);
    let (module, _) = env.process();
    assert!(module.ensure_instance_identity().await.unwrap().created);
}

#[derive(Clone, Copy, PartialEq)]
enum KeyState {
    Active,
    Retired,
    Revoked,
}

fn dummy_key(
    origin: &InstanceOrigin,
    state: KeyState,
    algorithm: SigningAlgorithm,
) -> InstanceSigningKey {
    let id = InstanceSigningKeyId::generate();
    InstanceSigningKey::new(
        id.clone(),
        1,
        algorithm,
        EncryptedPrivateKey {
            ciphertext: String::new(),
            nonce: String::new(),
            salt: String::new(),
            algorithm,
        },
        String::new(),
        origin.key_id_uri(&id),
        time::OffsetDateTime::now_utc(),
        (state == KeyState::Retired).then(time::OffsetDateTime::now_utc),
        (state == KeyState::Revoked).then(time::OffsetDateTime::now_utc),
    )
}

#[test]
fn key_selection_rules() {
    let origin = InstanceOrigin::parse(BASE).unwrap();
    let identity = InstanceIdentity::new(origin.clone(), time::OffsetDateTime::now_utc());
    let active = dummy_key(&origin, KeyState::Active, SigningAlgorithm::Rsa2048);
    let retired = dummy_key(&origin, KeyState::Retired, SigningAlgorithm::Rsa2048);
    let revoked = dummy_key(&origin, KeyState::Revoked, SigningAlgorithm::Rsa2048);

    // Retired and revoked keys never sign; only the active one is selected.
    let keys = [revoked.clone(), retired.clone(), active.clone()];
    assert_eq!(
        select_active_key(&origin, &identity, &keys).unwrap().id(),
        active.id()
    );
    assert!(active.is_active() && active.is_published());
    assert!(!retired.is_active() && retired.is_published());
    assert!(!revoked.is_active() && !revoked.is_published());

    for none in [
        vec![revoked.clone()],
        vec![retired.clone()],
        vec![revoked.clone(), retired.clone()],
    ] {
        assert_eq!(
            problem(&select_active_key(&origin, &identity, &none).unwrap_err()),
            Some(&InstanceIdentityProblem::NoActiveKey)
        );
    }
    let two = [
        active.clone(),
        dummy_key(&origin, KeyState::Active, SigningAlgorithm::Rsa2048),
    ];
    assert_eq!(
        problem(&select_active_key(&origin, &identity, &two).unwrap_err()),
        Some(&InstanceIdentityProblem::MultipleActiveKeys)
    );
    let ed = [dummy_key(
        &origin,
        KeyState::Active,
        SigningAlgorithm::Ed25519,
    )];
    assert_eq!(
        problem(&select_active_key(&origin, &identity, &ed).unwrap_err()),
        Some(&InstanceIdentityProblem::UnsupportedAlgorithm)
    );
    let other = InstanceOrigin::parse("https://other.example").unwrap();
    assert_eq!(
        problem(&select_active_key(&other, &identity, &keys).unwrap_err()),
        Some(&InstanceIdentityProblem::OriginMismatch)
    );
}

// --- cache ---------------------------------------------------------------

fn unlocked(id: &InstanceSigningKeyId, revision: i64) -> UnlockedInstanceSigningKey {
    UnlockedInstanceSigningKey {
        key_id: id.clone(),
        revision,
        key_id_uri: format!("https://emumet.example/ap/instance#key-{id}"),
        algorithm: SigningAlgorithm::Rsa2048,
        signer: Box::new(NullSigner),
    }
}

struct NullSigner;

impl PreparedSigner for NullSigner {
    fn sign(&self, _data: &[u8]) -> error_stack::Result<Vec<u8>, KernelError> {
        Ok(b"not-a-real-key".to_vec())
    }

    fn algorithm(&self) -> SigningAlgorithm {
        SigningAlgorithm::Rsa2048
    }
}

#[tokio::test]
async fn cache_hits_do_not_reload_and_misses_are_single_flight() {
    let cache = Arc::new(InstanceKeyCache::default());
    let id = InstanceSigningKeyId::generate();
    let loads = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let cache = cache.clone();
        let id = id.clone();
        let loads = loads.clone();
        tasks.push(tokio::spawn(async move {
            cache
                .get_or_try_load(&id, 1, || async {
                    loads.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Ok(unlocked(&id, 1))
                })
                .await
                .unwrap()
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cache_is_keyed_by_key_id_and_revision_and_bounded() {
    let cache = InstanceKeyCache::new(2, Duration::from_secs(900));
    let start = Instant::now();
    let a = InstanceSigningKeyId::generate();
    let b = InstanceSigningKeyId::generate();
    let loads = AtomicUsize::new(0);
    let load = |id: &InstanceSigningKeyId, revision: i64, at: Duration| {
        let id = id.clone();
        let loads = &loads;
        let cache = &cache;
        async move {
            cache
                .get_or_try_load_at(start + at, &id.clone(), revision, || async {
                    loads.fetch_add(1, Ordering::SeqCst);
                    Ok(unlocked(&id, revision))
                })
                .await
                .unwrap()
        }
    };
    load(&a, 1, Duration::from_secs(0)).await;
    load(&a, 1, Duration::from_secs(1)).await;
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    // Another revision of the same key id is a different entry.
    load(&a, 2, Duration::from_secs(2)).await;
    assert_eq!(loads.load(Ordering::SeqCst), 2);
    // A third entry evicts the oldest: the cap is 2.
    load(&b, 1, Duration::from_secs(3)).await;
    assert_eq!(cache.len().await, 2);
    load(&a, 1, Duration::from_secs(4)).await;
    assert_eq!(
        loads.load(Ordering::SeqCst),
        4,
        "the evicted entry is loaded again"
    );
    assert!(cache.len().await <= 2);
}

#[tokio::test]
async fn an_expired_entry_is_reloaded_and_a_failure_then_is_an_error_not_a_stale_key() {
    let ttl = Duration::from_secs(900);
    let cache = InstanceKeyCache::new(2, ttl);
    let start = Instant::now();
    let id = InstanceSigningKeyId::generate();
    cache
        .get_or_try_load_at(start, &id, 1, || async { Ok(unlocked(&id, 1)) })
        .await
        .unwrap();
    // Within the TTL the cached key is used without loading.
    cache
        .get_or_try_load_at(start + ttl - Duration::from_secs(1), &id, 1, || async {
            Err(Report::new(KernelError::Internal))
        })
        .await
        .unwrap();
    // After the TTL a failing load is an error and the old key is not used.
    let error = cache
        .get_or_try_load_at(start + ttl, &id, 1, || async {
            Err(Report::new(KernelError::Internal))
        })
        .await
        .unwrap_err();
    assert_eq!(error.current_context(), &KernelError::Internal);
    assert_eq!(cache.len().await, 0);
}

#[test]
fn unlocked_key_debug_never_prints_the_private_key() {
    let id = InstanceSigningKeyId::generate();
    let key = unlocked(&id, 1);
    let debug = format!("{key:?}");
    assert!(!debug.contains("not-a-real-key"));
    assert!(!debug.contains("NullSigner"));
    assert!(debug.contains("REDACTED"));
}
