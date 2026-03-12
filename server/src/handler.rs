use crate::applier::ApplierContainer;
use crate::hydra::HydraAdminClient;
use crate::kratos::KratosClient;
use adapter::processor::account::DependOnAccountSignal;
use adapter::processor::auth_account::DependOnAuthAccountSignal;
use adapter::processor::metadata::DependOnMetadataSignal;
use adapter::processor::profile::DependOnProfileSignal;
use driver::crypto::{
    Argon2Encryptor, FilePasswordProvider, Rsa2048RawGenerator, Rsa2048Signer, Rsa2048Verifier,
};
use driver::database::{PostgresDatabase, RedisDatabase};
use kernel::interfaces::crypto::{
    DependOnKeyEncryptor, DependOnPasswordProvider, DependOnRawKeyGenerator,
    DependOnSignatureVerifier, DependOnSigner,
};
use kernel::interfaces::database::DependOnDatabaseConnection;
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

    pub fn hydra_admin_client(&self) -> &HydraAdminClient {
        &self.handler.hydra_admin_client
    }

    pub fn kratos_client(&self) -> &KratosClient {
        &self.handler.kratos_client
    }
}

// --- DependOn* implementations for AppModule (delegate to handler/applier_container) ---

impl kernel::interfaces::database::DependOnDatabaseConnection for AppModule {
    type DatabaseConnection = PostgresDatabase;
    fn database_connection(&self) -> &Self::DatabaseConnection {
        self.handler.as_ref().database_connection()
    }
}

impl kernel::interfaces::read_model::DependOnAccountReadModel for AppModule {
    type AccountReadModel = <PostgresDatabase as kernel::interfaces::read_model::DependOnAccountReadModel>::AccountReadModel;
    fn account_read_model(&self) -> &Self::AccountReadModel {
        kernel::interfaces::read_model::DependOnAccountReadModel::account_read_model(
            self.handler.as_ref().database_connection(),
        )
    }
}

impl kernel::interfaces::event_store::DependOnAccountEventStore for AppModule {
    type AccountEventStore = <PostgresDatabase as kernel::interfaces::event_store::DependOnAccountEventStore>::AccountEventStore;
    fn account_event_store(&self) -> &Self::AccountEventStore {
        kernel::interfaces::event_store::DependOnAccountEventStore::account_event_store(
            self.handler.as_ref().database_connection(),
        )
    }
}

impl DependOnPasswordProvider for AppModule {
    type PasswordProvider = FilePasswordProvider;
    fn password_provider(&self) -> &Self::PasswordProvider {
        self.handler.as_ref().password_provider()
    }
}

impl DependOnRawKeyGenerator for AppModule {
    type RawKeyGenerator = Rsa2048RawGenerator;
    fn raw_key_generator(&self) -> &Self::RawKeyGenerator {
        self.handler.as_ref().raw_key_generator()
    }
}

impl DependOnKeyEncryptor for AppModule {
    type KeyEncryptor = Argon2Encryptor;
    fn key_encryptor(&self) -> &Self::KeyEncryptor {
        self.handler.as_ref().key_encryptor()
    }
}

impl DependOnAccountSignal for AppModule {
    type AccountSignal = ApplierContainer;
    fn account_signal(&self) -> &Self::AccountSignal {
        &self.applier_container
    }
}

impl DependOnAuthAccountSignal for AppModule {
    type AuthAccountSignal = ApplierContainer;
    fn auth_account_signal(&self) -> &Self::AuthAccountSignal {
        &self.applier_container
    }
}

impl kernel::interfaces::read_model::DependOnAuthAccountReadModel for AppModule {
    type AuthAccountReadModel = <PostgresDatabase as kernel::interfaces::read_model::DependOnAuthAccountReadModel>::AuthAccountReadModel;
    fn auth_account_read_model(&self) -> &Self::AuthAccountReadModel {
        kernel::interfaces::read_model::DependOnAuthAccountReadModel::auth_account_read_model(
            self.handler.as_ref().database_connection(),
        )
    }
}

impl kernel::interfaces::event_store::DependOnAuthAccountEventStore for AppModule {
    type AuthAccountEventStore = <PostgresDatabase as kernel::interfaces::event_store::DependOnAuthAccountEventStore>::AuthAccountEventStore;
    fn auth_account_event_store(&self) -> &Self::AuthAccountEventStore {
        kernel::interfaces::event_store::DependOnAuthAccountEventStore::auth_account_event_store(
            self.handler.as_ref().database_connection(),
        )
    }
}

impl kernel::interfaces::read_model::DependOnProfileReadModel for AppModule {
    type ProfileReadModel = <PostgresDatabase as kernel::interfaces::read_model::DependOnProfileReadModel>::ProfileReadModel;
    fn profile_read_model(&self) -> &Self::ProfileReadModel {
        kernel::interfaces::read_model::DependOnProfileReadModel::profile_read_model(
            self.handler.as_ref().database_connection(),
        )
    }
}

impl kernel::interfaces::event_store::DependOnProfileEventStore for AppModule {
    type ProfileEventStore = <PostgresDatabase as kernel::interfaces::event_store::DependOnProfileEventStore>::ProfileEventStore;
    fn profile_event_store(&self) -> &Self::ProfileEventStore {
        kernel::interfaces::event_store::DependOnProfileEventStore::profile_event_store(
            self.handler.as_ref().database_connection(),
        )
    }
}

impl DependOnProfileSignal for AppModule {
    type ProfileSignal = ApplierContainer;
    fn profile_signal(&self) -> &Self::ProfileSignal {
        &self.applier_container
    }
}

impl kernel::interfaces::read_model::DependOnMetadataReadModel for AppModule {
    type MetadataReadModel = <PostgresDatabase as kernel::interfaces::read_model::DependOnMetadataReadModel>::MetadataReadModel;
    fn metadata_read_model(&self) -> &Self::MetadataReadModel {
        kernel::interfaces::read_model::DependOnMetadataReadModel::metadata_read_model(
            self.handler.as_ref().database_connection(),
        )
    }
}

impl kernel::interfaces::event_store::DependOnMetadataEventStore for AppModule {
    type MetadataEventStore = <PostgresDatabase as kernel::interfaces::event_store::DependOnMetadataEventStore>::MetadataEventStore;
    fn metadata_event_store(&self) -> &Self::MetadataEventStore {
        kernel::interfaces::event_store::DependOnMetadataEventStore::metadata_event_store(
            self.handler.as_ref().database_connection(),
        )
    }
}

impl DependOnMetadataSignal for AppModule {
    type MetadataSignal = ApplierContainer;
    fn metadata_signal(&self) -> &Self::MetadataSignal {
        &self.applier_container
    }
}

impl kernel::interfaces::repository::DependOnAuthHostRepository for AppModule {
    type AuthHostRepository =
        <PostgresDatabase as kernel::interfaces::repository::DependOnAuthHostRepository>::AuthHostRepository;
    fn auth_host_repository(&self) -> &Self::AuthHostRepository {
        kernel::interfaces::repository::DependOnAuthHostRepository::auth_host_repository(
            self.handler.as_ref().database_connection(),
        )
    }
}

impl kernel::interfaces::repository::DependOnFollowRepository for AppModule {
    type FollowRepository =
        <PostgresDatabase as kernel::interfaces::repository::DependOnFollowRepository>::FollowRepository;
    fn follow_repository(&self) -> &Self::FollowRepository {
        kernel::interfaces::repository::DependOnFollowRepository::follow_repository(
            self.handler.as_ref().database_connection(),
        )
    }
}

impl kernel::interfaces::repository::DependOnRemoteAccountRepository for AppModule {
    type RemoteAccountRepository =
        <PostgresDatabase as kernel::interfaces::repository::DependOnRemoteAccountRepository>::RemoteAccountRepository;
    fn remote_account_repository(&self) -> &Self::RemoteAccountRepository {
        kernel::interfaces::repository::DependOnRemoteAccountRepository::remote_account_repository(
            self.handler.as_ref().database_connection(),
        )
    }
}

impl kernel::interfaces::repository::DependOnImageRepository for AppModule {
    type ImageRepository =
        <PostgresDatabase as kernel::interfaces::repository::DependOnImageRepository>::ImageRepository;
    fn image_repository(&self) -> &Self::ImageRepository {
        kernel::interfaces::repository::DependOnImageRepository::image_repository(
            self.handler.as_ref().database_connection(),
        )
    }
}

// Note: DependOnSigningKeyGenerator, DependOnAccountCommandProcessor,
// DependOnAccountQueryProcessor, and all UseCase traits are provided
// automatically via blanket impls in adapter.

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
    // Ory clients
    pub(crate) hydra_admin_client: HydraAdminClient,
    pub(crate) kratos_client: KratosClient,
}

impl Handler {
    pub async fn init() -> error_stack::Result<Self, KernelError> {
        let hydra_admin_url =
            dotenvy::var("HYDRA_ADMIN_URL").unwrap_or_else(|_| "http://localhost:4445".to_string());
        let kratos_public_url = dotenvy::var("KRATOS_PUBLIC_URL")
            .unwrap_or_else(|_| "http://localhost:4433".to_string());
        Self::init_with_urls(hydra_admin_url, kratos_public_url).await
    }

    async fn init_with_urls(
        hydra_admin_url: String,
        kratos_public_url: String,
    ) -> error_stack::Result<Self, KernelError> {
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
            hydra_admin_client: HydraAdminClient::new(hydra_admin_url),
            kratos_client: KratosClient::new(kratos_public_url),
        })
    }
}

#[cfg(test)]
impl AppModule {
    pub(crate) async fn new_for_oauth2_test(
        hydra_admin_url: String,
        kratos_public_url: String,
    ) -> error_stack::Result<Self, KernelError> {
        let handler = Arc::new(Handler::init_with_urls(hydra_admin_url, kratos_public_url).await?);
        let applier_container = Arc::new(ApplierContainer::new(handler.clone()));
        Ok(Self {
            handler,
            applier_container,
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
