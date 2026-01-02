use crate::applier::ApplierContainer;
use driver::crypto::{
    Argon2Encryptor, FilePasswordProvider, Rsa2048RawGenerator, Rsa2048Signer, Rsa2048Verifier,
};
use driver::database::{PostgresDatabase, RedisDatabase};
use kernel::interfaces::crypto::{
    DependOnKeyEncryptor, DependOnPasswordProvider, DependOnRawKeyGenerator,
    DependOnSignatureVerifier, DependOnSigner,
};
use kernel::KernelError;
use std::sync::Arc;
use vodca::References;

#[derive(Clone, References)]
pub struct AppModule {
    handler: Arc<Handler>,
    applier_container: Arc<ApplierContainer>,
}

impl AppModule {
    pub async fn new() -> error_stack::Result<Self, KernelError> {
        let handler = Arc::new(Handler::init().await?);
        let applier_container = Arc::new(ApplierContainer::new(handler.clone()));
        Ok(Self {
            handler,
            applier_container,
        })
    }
}

#[derive(References)]
pub struct Handler {
    pgpool: PostgresDatabase,
    redis: RedisDatabase,
    // Crypto providers
    password_provider: FilePasswordProvider,
    raw_key_generator: Rsa2048RawGenerator,
    key_encryptor: Argon2Encryptor,
    signer: Rsa2048Signer,
    verifier: Rsa2048Verifier,
}

impl Handler {
    pub async fn init() -> error_stack::Result<Self, KernelError> {
        let pgpool = PostgresDatabase::new().await?;
        let redis = RedisDatabase::new()?;

        Ok(Self {
            pgpool,
            redis,
            password_provider: FilePasswordProvider::new(),
            raw_key_generator: Rsa2048RawGenerator,
            key_encryptor: Argon2Encryptor::default(),
            signer: Rsa2048Signer,
            verifier: Rsa2048Verifier,
        })
    }
}

// --- Database DI implementations (via macro) ---

kernel::impl_database_delegation!(Handler, pgpool, PostgresDatabase);

// --- Crypto DI implementations ---

impl DependOnPasswordProvider for Handler {
    type PasswordProvider = FilePasswordProvider;
    fn password_provider(&self) -> &Self::PasswordProvider {
        &self.password_provider
    }
}

impl DependOnRawKeyGenerator for Handler {
    type RawKeyGenerator = Rsa2048RawGenerator;
    fn raw_key_generator(&self) -> &Self::RawKeyGenerator {
        &self.raw_key_generator
    }
}

impl DependOnKeyEncryptor for Handler {
    type KeyEncryptor = Argon2Encryptor;
    fn key_encryptor(&self) -> &Self::KeyEncryptor {
        &self.key_encryptor
    }
}

// Note: DependOnSigningKeyGenerator is provided automatically via blanket impl in adapter
// when Handler implements DependOnRawKeyGenerator + DependOnKeyEncryptor

impl DependOnSigner for Handler {
    type Signer = Rsa2048Signer;
    fn signer(&self) -> &Self::Signer {
        &self.signer
    }
}

impl DependOnSignatureVerifier for Handler {
    type SignatureVerifier = Rsa2048Verifier;
    fn signature_verifier(&self) -> &Self::SignatureVerifier {
        &self.verifier
    }
}
