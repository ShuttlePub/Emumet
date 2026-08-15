use crate::hydra::HydraAdminClient;
use crate::kratos::KratosClient;
use driver::crypto::{
    Argon2Encryptor, FilePasswordProvider, Rsa2048RawGenerator, Rsa2048Signer, Rsa2048Verifier,
};
use driver::database::PostgresDatabase;
use driver::http_signing::{HttpSignatureVerifierImpl, HttpSignerImpl};
use driver::keto::KetoClient;
use kernel::interfaces::config::{DependOnPublicBaseUrl, PublicBaseUrl};
use kernel::interfaces::crypto::{
    DependOnKeyEncryptor, DependOnPasswordProvider, DependOnRawKeyGenerator,
    DependOnSignatureVerifier, DependOnSigner,
};
use kernel::interfaces::http_signing::{DependOnHttpSignatureVerifier, DependOnHttpSigner};
use kernel::interfaces::permission::{DependOnPermissionChecker, DependOnPermissionWriter};
use kernel::KernelError;

/// Single wiring root for the HTTP server (ADR 0006 decision 7).
///
/// Database delegation goes through `kernel::impl_database_delegation!`;
/// non-database infrastructure is wired by the small handwritten impls below.
/// No use-case logic lives here.
#[derive(Clone)]
pub struct AppModule {
    pgpool: PostgresDatabase,
    password_provider: FilePasswordProvider,
    raw_key_generator: Rsa2048RawGenerator,
    key_encryptor: Argon2Encryptor,
    signer: Rsa2048Signer,
    verifier: Rsa2048Verifier,
    http_signer: HttpSignerImpl,
    http_signature_verifier: HttpSignatureVerifierImpl,
    public_base_url: PublicBaseUrl,
    hydra_admin_client: HydraAdminClient,
    kratos_client: KratosClient,
    keto_client: KetoClient,
}

impl AppModule {
    pub async fn new() -> error_stack::Result<Self, KernelError> {
        let hydra_admin_url =
            dotenvy::var("HYDRA_ADMIN_URL").unwrap_or_else(|_| "http://localhost:4445".to_string());
        let kratos_public_url = dotenvy::var("KRATOS_PUBLIC_URL")
            .unwrap_or_else(|_| "http://localhost:4433".to_string());
        let keto_read_url =
            dotenvy::var("KETO_READ_URL").unwrap_or_else(|_| "http://localhost:4466".to_string());
        let keto_write_url =
            dotenvy::var("KETO_WRITE_URL").unwrap_or_else(|_| "http://localhost:4467".to_string());
        let public_base_url =
            dotenvy::var("PUBLIC_BASE_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());

        let pgpool = PostgresDatabase::new().await?;

        Ok(Self {
            pgpool,
            password_provider: FilePasswordProvider::new(),
            raw_key_generator: Rsa2048RawGenerator,
            key_encryptor: Argon2Encryptor::default(),
            signer: Rsa2048Signer,
            verifier: Rsa2048Verifier,
            http_signer: HttpSignerImpl,
            http_signature_verifier: HttpSignatureVerifierImpl::new()?,
            public_base_url: PublicBaseUrl::new(public_base_url),
            hydra_admin_client: HydraAdminClient::new(hydra_admin_url),
            kratos_client: KratosClient::new(kratos_public_url),
            keto_client: KetoClient::new(keto_read_url, keto_write_url),
        })
    }

    #[cfg(test)]
    pub(crate) async fn new_for_test_urls(
        hydra_admin_url: String,
        kratos_public_url: String,
        keto_read_url: String,
        keto_write_url: String,
    ) -> error_stack::Result<Self, KernelError> {
        let pgpool = PostgresDatabase::new().await?;
        let public_base_url =
            dotenvy::var("PUBLIC_BASE_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
        Ok(Self {
            pgpool,
            password_provider: FilePasswordProvider::new(),
            raw_key_generator: Rsa2048RawGenerator,
            key_encryptor: Argon2Encryptor::default(),
            signer: Rsa2048Signer,
            verifier: Rsa2048Verifier,
            http_signer: HttpSignerImpl,
            http_signature_verifier: HttpSignatureVerifierImpl::new()?,
            public_base_url: PublicBaseUrl::new(public_base_url),
            hydra_admin_client: HydraAdminClient::new(hydra_admin_url),
            kratos_client: KratosClient::new(kratos_public_url),
            keto_client: KetoClient::new(keto_read_url, keto_write_url),
        })
    }

    #[cfg(test)]
    pub(crate) async fn new_for_oauth2_test(
        hydra_admin_url: String,
        kratos_public_url: String,
    ) -> error_stack::Result<Self, KernelError> {
        let keto_read_url =
            dotenvy::var("KETO_READ_URL").unwrap_or_else(|_| "http://localhost:4466".to_string());
        let keto_write_url =
            dotenvy::var("KETO_WRITE_URL").unwrap_or_else(|_| "http://localhost:4467".to_string());
        Self::new_for_test_urls(
            hydra_admin_url,
            kratos_public_url,
            keto_read_url,
            keto_write_url,
        )
        .await
    }

    pub fn hydra_admin_client(&self) -> &HydraAdminClient {
        &self.hydra_admin_client
    }

    pub fn kratos_client(&self) -> &KratosClient {
        &self.kratos_client
    }
}

kernel::impl_database_delegation!(AppModule, pgpool, PostgresDatabase);

impl DependOnPasswordProvider for AppModule {
    type PasswordProvider = FilePasswordProvider;
    fn password_provider(&self) -> &Self::PasswordProvider {
        &self.password_provider
    }
}

impl DependOnRawKeyGenerator for AppModule {
    type RawKeyGenerator = Rsa2048RawGenerator;
    fn raw_key_generator(&self) -> &Self::RawKeyGenerator {
        &self.raw_key_generator
    }
}

impl DependOnKeyEncryptor for AppModule {
    type KeyEncryptor = Argon2Encryptor;
    fn key_encryptor(&self) -> &Self::KeyEncryptor {
        &self.key_encryptor
    }
}

impl DependOnSigner for AppModule {
    type Signer = Rsa2048Signer;
    fn signer(&self) -> &Self::Signer {
        &self.signer
    }
}

impl DependOnSignatureVerifier for AppModule {
    type SignatureVerifier = Rsa2048Verifier;
    fn signature_verifier(&self) -> &Self::SignatureVerifier {
        &self.verifier
    }
}

impl DependOnPermissionChecker for AppModule {
    type PermissionChecker = KetoClient;
    fn permission_checker(&self) -> &Self::PermissionChecker {
        &self.keto_client
    }
}

impl DependOnPermissionWriter for AppModule {
    type PermissionWriter = KetoClient;
    fn permission_writer(&self) -> &Self::PermissionWriter {
        &self.keto_client
    }
}

impl DependOnHttpSigner for AppModule {
    type HttpSigner = HttpSignerImpl;
    fn http_signer(&self) -> &Self::HttpSigner {
        &self.http_signer
    }
}

impl DependOnHttpSignatureVerifier for AppModule {
    type HttpSignatureVerifier = HttpSignatureVerifierImpl;
    fn http_signature_verifier(&self) -> &Self::HttpSignatureVerifier {
        &self.http_signature_verifier
    }
}

impl DependOnPublicBaseUrl for AppModule {
    fn public_base_url(&self) -> &PublicBaseUrl {
        &self.public_base_url
    }
}
