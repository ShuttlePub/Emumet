use crate::hydra::HydraAdminClient;
use crate::kratos::{KratosClient, KratosRegistrationFlowDirectory};
use driver::crypto::{
    Argon2Encryptor, FilePasswordProvider, Rsa2048RawGenerator, Rsa2048Signer, Rsa2048Verifier,
};
use driver::database::PostgresDatabase;
use driver::http_signing::{HttpSignatureVerifierImpl, HttpSignerImpl};
use driver::keto::KetoClient;
#[cfg(test)]
use driver::registration_protection::SmtpSettings;
use driver::registration_protection::{HttpTurnstileVerifier, SmtpEmailTransport};
use driver::storage::{NoopProfileMediaCopyGateway, S3ImageStorage};
use driver::webhook::HttpWebhookSender;
use kernel::interfaces::config::{DependOnPublicBaseUrl, PublicBaseUrl};
use kernel::interfaces::crypto::{
    DependOnKeyEncryptor, DependOnPasswordProvider, DependOnRawKeyGenerator,
    DependOnSignatureVerifier, DependOnSigner,
};
use kernel::interfaces::http_signing::{DependOnHttpSignatureVerifier, DependOnHttpSigner};
use kernel::interfaces::permission::{DependOnPermissionChecker, DependOnPermissionWriter};
use kernel::interfaces::repository::{
    DependOnEmailTransport, DependOnRegistrationFlowDirectory,
    DependOnRegistrationProtectionPolicy, DependOnTurnstileVerifier, DependOnWebhookSender,
};
use kernel::prelude::entity::{EmailChallengePolicy, ReadinessPolicy};
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
    image_storage: S3ImageStorage,
    profile_media_copy_gateway: NoopProfileMediaCopyGateway,
    webhook_sender: HttpWebhookSender,
    turnstile_verifier: HttpTurnstileVerifier,
    email_transport: SmtpEmailTransport,
    registration_flow_directory: KratosRegistrationFlowDirectory,
    readiness_policy: ReadinessPolicy,
    email_challenge_policy: EmailChallengePolicy,
}

fn positive_secs_from_env(key: &str) -> Option<i64> {
    dotenvy::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
}

/// 鮮度・TTL・rate limit のうち運用で調整する値だけ環境変数で上書きできる (既定値と根拠は
/// docs/registration-protection-readiness.md)。
fn registration_protection_policies_from_env() -> (ReadinessPolicy, EmailChallengePolicy) {
    let mut readiness = ReadinessPolicy::default();
    if let Some(secs) = positive_secs_from_env("REGISTRATION_PROTECTION_SUCCESS_TTL_SECS") {
        readiness.success_ttl = time::Duration::seconds(secs);
    }
    if let Some(secs) = positive_secs_from_env("REGISTRATION_PROTECTION_PROBE_MAX_AGE_SECS") {
        readiness.probe_max_age = time::Duration::seconds(secs);
    }
    let mut email = EmailChallengePolicy::default();
    if let Some(secs) = positive_secs_from_env("REGISTRATION_PROOF_REUSE_WINDOW_SECS") {
        email.proof_reuse_window = time::Duration::seconds(secs);
    }
    if let Some(limit) = positive_secs_from_env("REGISTRATION_EMAIL_MAX_PER_SOURCE_HOUR") {
        email.limits.source_per_hour = limit;
    }
    (readiness, email)
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
        let image_storage = S3ImageStorage::from_env().await?;
        let (readiness_policy, email_challenge_policy) =
            registration_protection_policies_from_env();
        let registration_flow_directory = KratosRegistrationFlowDirectory::new(&kratos_public_url);

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
            image_storage,
            profile_media_copy_gateway: NoopProfileMediaCopyGateway,
            webhook_sender: HttpWebhookSender::from_env(),
            turnstile_verifier: HttpTurnstileVerifier::from_env(),
            email_transport: SmtpEmailTransport::from_env(),
            registration_flow_directory,
            readiness_policy,
            email_challenge_policy,
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
        Self::new_for_test_database(
            pgpool,
            hydra_admin_url,
            kratos_public_url,
            keto_read_url,
            keto_write_url,
        )
        .await
    }

    /// Same test wiring on an explicit database (the P2 social graph tables are not part of the
    /// production migrations, so their tests run on an isolated database that has the fixture).
    #[cfg(test)]
    pub(crate) async fn new_for_test_database(
        pgpool: PostgresDatabase,
        hydra_admin_url: String,
        kratos_public_url: String,
        keto_read_url: String,
        keto_write_url: String,
    ) -> error_stack::Result<Self, KernelError> {
        let image_storage = S3ImageStorage::from_env().await?;
        let public_base_url =
            dotenvy::var("PUBLIC_BASE_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
        let registration_flow_directory = KratosRegistrationFlowDirectory::new(&kratos_public_url);
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
            image_storage,
            profile_media_copy_gateway: NoopProfileMediaCopyGateway,
            webhook_sender: HttpWebhookSender::disabled(),
            turnstile_verifier: HttpTurnstileVerifier::new(
                driver::registration_protection::turnstile::TurnstileSettings::from_source(
                    &driver::registration_protection::EnvSource::new(),
                ),
            ),
            email_transport: SmtpEmailTransport::new(SmtpSettings::from_source(
                &driver::registration_protection::EnvSource::new(),
            )),
            registration_flow_directory,
            readiness_policy: ReadinessPolicy::default(),
            email_challenge_policy: EmailChallengePolicy::default(),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_webhook_sender(mut self, webhook_sender: HttpWebhookSender) -> Self {
        self.webhook_sender = webhook_sender;
        self
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

impl kernel::interfaces::storage::DependOnImageStorage for AppModule {
    type ImageStorage = S3ImageStorage;

    fn image_storage(&self) -> &Self::ImageStorage {
        &self.image_storage
    }
}

impl kernel::interfaces::storage::DependOnProfileMediaCopyGateway for AppModule {
    type ProfileMediaCopyGateway = NoopProfileMediaCopyGateway;

    fn profile_media_copy_gateway(&self) -> &Self::ProfileMediaCopyGateway {
        &self.profile_media_copy_gateway
    }
}

impl DependOnWebhookSender for AppModule {
    type WebhookSender = HttpWebhookSender;

    fn webhook_sender(&self) -> &Self::WebhookSender {
        &self.webhook_sender
    }
}

impl DependOnTurnstileVerifier for AppModule {
    type TurnstileVerifier = HttpTurnstileVerifier;

    fn turnstile_verifier(&self) -> &Self::TurnstileVerifier {
        &self.turnstile_verifier
    }
}

impl DependOnEmailTransport for AppModule {
    type EmailTransport = SmtpEmailTransport;

    fn email_transport(&self) -> &Self::EmailTransport {
        &self.email_transport
    }
}

impl DependOnRegistrationFlowDirectory for AppModule {
    type RegistrationFlowDirectory = KratosRegistrationFlowDirectory;

    fn registration_flow_directory(&self) -> &Self::RegistrationFlowDirectory {
        &self.registration_flow_directory
    }
}

impl DependOnRegistrationProtectionPolicy for AppModule {
    fn readiness_policy(&self) -> ReadinessPolicy {
        self.readiness_policy
    }

    fn email_challenge_policy(&self) -> EmailChallengePolicy {
        self.email_challenge_policy
    }
}
