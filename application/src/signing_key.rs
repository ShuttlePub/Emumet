use adapter::crypto::{DependOnSigningKeyGenerator, SigningKeyGenerator};
use error_stack::Report;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::crypto::{
    DependOnKeyEncryptor, DependOnPasswordProvider, KeyEncryptor, PasswordProvider,
    SigningAlgorithm,
};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::http_signing::{
    DependOnHttpSigner, HttpSigner, HttpSigningRequest, HttpSigningResponse,
};
use kernel::interfaces::repository::DependOnSigningKeyRepository;
use kernel::prelude::entity::{
    Account, AccountId, Nanoid, SigningKey, SigningKeyId, SigningKeyRepository,
};
use kernel::KernelError;
use std::future::Future;

#[derive(Debug, Clone)]
pub struct PublicKeyInfo {
    pub id: String,
    pub owner: String,
    pub public_key_pem: String,
}

pub trait CreateSigningKeyUseCase:
    'static
    + Sync
    + Send
    + DependOnSigningKeyRepository
    + DependOnSigningKeyGenerator
    + DependOnPublicBaseUrl
    + DependOnPasswordProvider
{
    fn create(
        &self,
        account_id: AccountId,
        nanoid: &Nanoid<Account>,
        algorithm: SigningAlgorithm,
    ) -> impl Future<Output = error_stack::Result<SigningKey, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().get_executor().await?;
            let password = self.password_provider().get_password()?;
            let key_pair = self.signing_key_generator().generate(&password)?;
            let base_url = self.public_base_url().as_str();
            let key_id_uri = format!("{}/accounts/{}#main-key", base_url, nanoid.as_ref());
            let signing_key = SigningKey::new(
                SigningKeyId::default(),
                account_id,
                algorithm,
                key_pair.encrypted_private_key,
                key_pair.public_key_pem,
                key_id_uri,
                time::OffsetDateTime::now_utc(),
                None,
            );
            self.signing_key_repository()
                .create(&mut executor, &signing_key)
                .await?;
            Ok(signing_key)
        }
    }
}

impl<T: ?Sized> CreateSigningKeyUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnSigningKeyRepository
        + DependOnSigningKeyGenerator
        + DependOnPublicBaseUrl
        + DependOnPasswordProvider
{
}

pub trait GetPublicKeyUseCase:
    'static + Sync + Send + DependOnSigningKeyRepository + DependOnPublicBaseUrl
{
    fn get_public_key_info(
        &self,
        account_id: &AccountId,
        nanoid: &Nanoid<Account>,
    ) -> impl Future<Output = error_stack::Result<PublicKeyInfo, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().get_executor().await?;
            let keys = self
                .signing_key_repository()
                .find_active_by_account_id(&mut executor, account_id)
                .await?;
            let key = keys.into_iter().next().ok_or_else(|| {
                Report::new(KernelError::NotFound)
                    .attach_printable("No active signing key found for account")
            })?;
            let base_url = self.public_base_url().as_str();
            let owner = format!("{}/accounts/{}", base_url, nanoid.as_ref());
            Ok(PublicKeyInfo {
                id: key.key_id_uri,
                owner,
                public_key_pem: key.public_key_pem,
            })
        }
    }
}

impl<T> GetPublicKeyUseCase for T where
    T: 'static + Sync + Send + DependOnSigningKeyRepository + DependOnPublicBaseUrl
{
}

pub trait SignRequestUseCase:
    'static
    + Sync
    + Send
    + DependOnSigningKeyRepository
    + DependOnHttpSigner
    + DependOnPasswordProvider
    + DependOnKeyEncryptor
{
    fn sign(
        &self,
        account_id: &AccountId,
        request: HttpSigningRequest,
    ) -> impl Future<Output = error_stack::Result<HttpSigningResponse, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().get_executor().await?;
            let keys = self
                .signing_key_repository()
                .find_active_by_account_id(&mut executor, account_id)
                .await?;
            let key = keys.into_iter().next().ok_or_else(|| {
                Report::new(KernelError::NotFound)
                    .attach_printable("No active signing key found for account")
            })?;
            let password = self.password_provider().get_password()?;
            let private_key_pem = self
                .key_encryptor()
                .decrypt(key.encrypted_private_key(), &password)?;
            self.http_signer()
                .sign(&request, &private_key_pem, &key.key_id_uri, key.algorithm())
                .await
        }
    }
}

impl<T> SignRequestUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnSigningKeyRepository
        + DependOnHttpSigner
        + DependOnPasswordProvider
        + DependOnKeyEncryptor
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::interfaces::config::PublicBaseUrl;
    use kernel::interfaces::crypto::{
        EncryptedPrivateKey, KeyEncryptor, PasswordProvider, RawKeyGenerator, SigningAlgorithm,
    };
    use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
    use kernel::interfaces::http_signing::{HttpSigner, HttpSigningRequest, HttpSigningResponse};
    use kernel::prelude::entity::{SigningKey, SigningKeyId};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use zeroize::Zeroizing;

    struct MockExecutor;
    impl Executor for MockExecutor {}

    struct MockDatabaseConnection;
    impl DatabaseConnection for MockDatabaseConnection {
        type Executor = MockExecutor;
        fn get_executor(
            &self,
        ) -> impl Future<Output = error_stack::Result<Self::Executor, KernelError>> + Send {
            async { Ok(MockExecutor) }
        }
    }

    struct MockSigningKeyRepository {
        created_keys: Arc<Mutex<Vec<SigningKey>>>,
        active_keys: Arc<Mutex<Vec<SigningKey>>>,
    }

    impl MockSigningKeyRepository {
        fn new() -> Self {
            Self {
                created_keys: Arc::new(Mutex::new(Vec::new())),
                active_keys: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn with_active_keys(keys: Vec<SigningKey>) -> Self {
            Self {
                created_keys: Arc::new(Mutex::new(Vec::new())),
                active_keys: Arc::new(Mutex::new(keys)),
            }
        }
    }

    impl kernel::prelude::entity::SigningKeyRepository for MockSigningKeyRepository {
        type Executor = MockExecutor;

        fn find_by_id(
            &self,
            _executor: &mut Self::Executor,
            _id: &SigningKeyId,
        ) -> impl Future<Output = error_stack::Result<SigningKey, KernelError>> + Send {
            async { Err(Report::new(KernelError::NotFound)) }
        }

        fn find_by_account_id(
            &self,
            _executor: &mut Self::Executor,
            _account_id: &AccountId,
        ) -> impl Future<Output = error_stack::Result<Vec<SigningKey>, KernelError>> + Send
        {
            async { Ok(vec![]) }
        }

        fn find_active_by_account_id(
            &self,
            _executor: &mut Self::Executor,
            _account_id: &AccountId,
        ) -> impl Future<Output = error_stack::Result<Vec<SigningKey>, KernelError>> + Send
        {
            let keys = self.active_keys.lock().unwrap().clone();
            async move { Ok(keys) }
        }

        fn create(
            &self,
            _executor: &mut Self::Executor,
            signing_key: &SigningKey,
        ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
            self.created_keys.lock().unwrap().push(signing_key.clone());
            async { Ok(()) }
        }

        fn revoke(
            &self,
            _executor: &mut Self::Executor,
            _id: &SigningKeyId,
        ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
            async { Ok(()) }
        }
    }

    struct MockPasswordProvider;

    impl PasswordProvider for MockPasswordProvider {
        fn get_password(&self) -> error_stack::Result<Zeroizing<Vec<u8>>, KernelError> {
            Ok(Zeroizing::new(b"test-password".to_vec()))
        }
    }

    struct MockRawKeyGenerator;

    impl RawKeyGenerator for MockRawKeyGenerator {
        fn generate_raw(
            &self,
        ) -> error_stack::Result<kernel::interfaces::crypto::RawKeyPair, KernelError> {
            Ok(kernel::interfaces::crypto::RawKeyPair {
                public_key_pem: "mock-public-key-pem".to_string(),
                private_key_pem: Zeroizing::new(b"mock-private-key-pem".to_vec()),
                algorithm: SigningAlgorithm::Rsa2048,
            })
        }

        fn algorithm(&self) -> SigningAlgorithm {
            SigningAlgorithm::Rsa2048
        }
    }

    struct MockKeyEncryptor;

    impl KeyEncryptor for MockKeyEncryptor {
        fn encrypt(
            &self,
            _private_key_pem: &[u8],
            _password: &[u8],
            algorithm: SigningAlgorithm,
        ) -> error_stack::Result<EncryptedPrivateKey, KernelError> {
            Ok(EncryptedPrivateKey {
                ciphertext: "mock-ciphertext".to_string(),
                nonce: "mock-nonce".to_string(),
                salt: "mock-salt".to_string(),
                algorithm,
            })
        }

        fn decrypt(
            &self,
            _encrypted: &EncryptedPrivateKey,
            _password: &[u8],
        ) -> error_stack::Result<Zeroizing<Vec<u8>>, KernelError> {
            Ok(Zeroizing::new(b"decrypted-private-key".to_vec()))
        }
    }

    struct MockHttpSigner;

    impl HttpSigner for MockHttpSigner {
        fn sign(
            &self,
            _request: &HttpSigningRequest,
            _private_key_pem: &[u8],
            _key_id: &str,
            _algorithm: &SigningAlgorithm,
        ) -> impl Future<Output = error_stack::Result<HttpSigningResponse, KernelError>> + Send
        {
            async {
                let mut cavage = HashMap::new();
                cavage.insert("signature".to_string(), "mock-cavage-sig".to_string());
                let mut rfc9421 = HashMap::new();
                rfc9421.insert("signature".to_string(), "mock-rfc9421-sig".to_string());
                rfc9421.insert(
                    "signature-input".to_string(),
                    "mock-rfc9421-input".to_string(),
                );
                Ok(HttpSigningResponse {
                    cavage_headers: cavage,
                    rfc9421_headers: rfc9421,
                })
            }
        }
    }

    struct CreateMockModule {
        db: MockDatabaseConnection,
        repo: MockSigningKeyRepository,
        password_provider: MockPasswordProvider,
        raw_key_generator: MockRawKeyGenerator,
        key_encryptor: MockKeyEncryptor,
        base_url: PublicBaseUrl,
    }

    impl CreateMockModule {
        fn new() -> Self {
            Self {
                db: MockDatabaseConnection,
                repo: MockSigningKeyRepository::new(),
                password_provider: MockPasswordProvider,
                raw_key_generator: MockRawKeyGenerator,
                key_encryptor: MockKeyEncryptor,
                base_url: PublicBaseUrl::new("https://example.com".to_string()),
            }
        }
    }

    impl DependOnDatabaseConnection for CreateMockModule {
        type DatabaseConnection = MockDatabaseConnection;
        fn database_connection(&self) -> &Self::DatabaseConnection {
            &self.db
        }
    }

    impl DependOnSigningKeyRepository for CreateMockModule {
        type SigningKeyRepository = MockSigningKeyRepository;
        fn signing_key_repository(&self) -> &Self::SigningKeyRepository {
            &self.repo
        }
    }

    impl DependOnPasswordProvider for CreateMockModule {
        type PasswordProvider = MockPasswordProvider;
        fn password_provider(&self) -> &Self::PasswordProvider {
            &self.password_provider
        }
    }

    impl kernel::interfaces::crypto::DependOnRawKeyGenerator for CreateMockModule {
        type RawKeyGenerator = MockRawKeyGenerator;
        fn raw_key_generator(&self) -> &Self::RawKeyGenerator {
            &self.raw_key_generator
        }
    }

    impl DependOnKeyEncryptor for CreateMockModule {
        type KeyEncryptor = MockKeyEncryptor;
        fn key_encryptor(&self) -> &Self::KeyEncryptor {
            &self.key_encryptor
        }
    }

    impl DependOnPublicBaseUrl for CreateMockModule {
        fn public_base_url(&self) -> &PublicBaseUrl {
            &self.base_url
        }
    }

    struct GetPublicKeyMockModule {
        db: MockDatabaseConnection,
        repo: MockSigningKeyRepository,
        base_url: PublicBaseUrl,
    }

    impl DependOnDatabaseConnection for GetPublicKeyMockModule {
        type DatabaseConnection = MockDatabaseConnection;
        fn database_connection(&self) -> &Self::DatabaseConnection {
            &self.db
        }
    }

    impl DependOnSigningKeyRepository for GetPublicKeyMockModule {
        type SigningKeyRepository = MockSigningKeyRepository;
        fn signing_key_repository(&self) -> &Self::SigningKeyRepository {
            &self.repo
        }
    }

    impl DependOnPublicBaseUrl for GetPublicKeyMockModule {
        fn public_base_url(&self) -> &PublicBaseUrl {
            &self.base_url
        }
    }

    struct SignMockModule {
        db: MockDatabaseConnection,
        repo: MockSigningKeyRepository,
        password_provider: MockPasswordProvider,
        key_encryptor: MockKeyEncryptor,
        http_signer: MockHttpSigner,
    }

    impl DependOnDatabaseConnection for SignMockModule {
        type DatabaseConnection = MockDatabaseConnection;
        fn database_connection(&self) -> &Self::DatabaseConnection {
            &self.db
        }
    }

    impl DependOnSigningKeyRepository for SignMockModule {
        type SigningKeyRepository = MockSigningKeyRepository;
        fn signing_key_repository(&self) -> &Self::SigningKeyRepository {
            &self.repo
        }
    }

    impl DependOnPasswordProvider for SignMockModule {
        type PasswordProvider = MockPasswordProvider;
        fn password_provider(&self) -> &Self::PasswordProvider {
            &self.password_provider
        }
    }

    impl DependOnKeyEncryptor for SignMockModule {
        type KeyEncryptor = MockKeyEncryptor;
        fn key_encryptor(&self) -> &Self::KeyEncryptor {
            &self.key_encryptor
        }
    }

    impl DependOnHttpSigner for SignMockModule {
        type HttpSigner = MockHttpSigner;
        fn http_signer(&self) -> &Self::HttpSigner {
            &self.http_signer
        }
    }

    #[tokio::test]
    async fn test_create_signing_key_success() {
        kernel::id::ensure_generator_initialized();
        let module = CreateMockModule::new();
        let account_id = AccountId::default();
        let nanoid = Nanoid::<Account>::new("abc123".to_string());

        let result = module
            .create(account_id.clone(), &nanoid, SigningAlgorithm::Rsa2048)
            .await;

        assert!(result.is_ok());
        let signing_key = result.unwrap();
        assert_eq!(
            signing_key.key_id_uri,
            "https://example.com/accounts/abc123#main-key"
        );
        assert_eq!(signing_key.public_key_pem, "mock-public-key-pem");
        assert_eq!(signing_key.revoked_at, None);
        assert_eq!(*signing_key.account_id(), account_id);

        let created = module.repo.created_keys.lock().unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].key_id_uri, signing_key.key_id_uri);
    }

    #[tokio::test]
    async fn test_create_signing_key_builds_correct_key_id_uri() {
        kernel::id::ensure_generator_initialized();
        let mut module = CreateMockModule::new();
        module.base_url = PublicBaseUrl::new("https://my-instance.social".to_string());
        let account_id = AccountId::default();
        let nanoid = Nanoid::<Account>::new("xyz789".to_string());

        let result = module
            .create(account_id, &nanoid, SigningAlgorithm::Rsa2048)
            .await;

        let signing_key = result.unwrap();
        assert_eq!(
            signing_key.key_id_uri,
            "https://my-instance.social/accounts/xyz789#main-key"
        );
    }

    #[tokio::test]
    async fn test_get_public_key_info_success() {
        kernel::id::ensure_generator_initialized();
        let account_id = AccountId::default();
        let active_key = SigningKey::new(
            SigningKeyId::default(),
            account_id.clone(),
            SigningAlgorithm::Rsa2048,
            EncryptedPrivateKey {
                ciphertext: "enc".to_string(),
                nonce: "nonce".to_string(),
                salt: "salt".to_string(),
                algorithm: SigningAlgorithm::Rsa2048,
            },
            "test-public-key-pem".to_string(),
            "https://example.com/accounts/abc123#main-key".to_string(),
            time::OffsetDateTime::now_utc(),
            None,
        );

        let module = GetPublicKeyMockModule {
            db: MockDatabaseConnection,
            repo: MockSigningKeyRepository::with_active_keys(vec![active_key]),
            base_url: PublicBaseUrl::new("https://example.com".to_string()),
        };

        let nanoid = Nanoid::<Account>::new("abc123".to_string());
        let result = module.get_public_key_info(&account_id, &nanoid).await;

        assert!(result.is_ok());
        let info = result.unwrap();
        assert_eq!(info.id, "https://example.com/accounts/abc123#main-key");
        assert_eq!(info.owner, "https://example.com/accounts/abc123");
        assert_eq!(info.public_key_pem, "test-public-key-pem");
    }

    #[tokio::test]
    async fn test_get_public_key_info_not_found() {
        kernel::id::ensure_generator_initialized();
        let account_id = AccountId::default();
        let module = GetPublicKeyMockModule {
            db: MockDatabaseConnection,
            repo: MockSigningKeyRepository::new(), // empty — no active keys
            base_url: PublicBaseUrl::new("https://example.com".to_string()),
        };

        let nanoid = Nanoid::<Account>::new("missing".to_string());
        let result = module.get_public_key_info(&account_id, &nanoid).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err.current_context(), KernelError::NotFound));
    }

    #[tokio::test]
    async fn test_sign_request_success() {
        kernel::id::ensure_generator_initialized();
        let account_id = AccountId::default();
        let active_key = SigningKey::new(
            SigningKeyId::default(),
            account_id.clone(),
            SigningAlgorithm::Rsa2048,
            EncryptedPrivateKey {
                ciphertext: "enc".to_string(),
                nonce: "nonce".to_string(),
                salt: "salt".to_string(),
                algorithm: SigningAlgorithm::Rsa2048,
            },
            "test-public-key-pem".to_string(),
            "https://example.com/accounts/abc123#main-key".to_string(),
            time::OffsetDateTime::now_utc(),
            None,
        );

        let module = SignMockModule {
            db: MockDatabaseConnection,
            repo: MockSigningKeyRepository::with_active_keys(vec![active_key]),
            password_provider: MockPasswordProvider,
            key_encryptor: MockKeyEncryptor,
            http_signer: MockHttpSigner,
        };

        let request = HttpSigningRequest {
            method: "POST".to_string(),
            url: "https://remote.example.com/inbox".to_string(),
            headers: HashMap::new(),
            body: Some(b"test body".to_vec()),
        };

        let result = module.sign(&account_id, request).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.cavage_headers.contains_key("signature"));
        assert!(response.rfc9421_headers.contains_key("signature"));
        assert!(response.rfc9421_headers.contains_key("signature-input"));
    }

    #[tokio::test]
    async fn test_sign_request_no_active_key() {
        kernel::id::ensure_generator_initialized();
        let account_id = AccountId::default();
        let module = SignMockModule {
            db: MockDatabaseConnection,
            repo: MockSigningKeyRepository::new(), // no active keys
            password_provider: MockPasswordProvider,
            key_encryptor: MockKeyEncryptor,
            http_signer: MockHttpSigner,
        };

        let request = HttpSigningRequest {
            method: "GET".to_string(),
            url: "https://remote.example.com/users/bob".to_string(),
            headers: HashMap::new(),
            body: None,
        };

        let result = module.sign(&account_id, request).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err.current_context(), KernelError::NotFound));
    }
}
