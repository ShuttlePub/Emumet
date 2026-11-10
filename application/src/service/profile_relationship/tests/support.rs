use crate::profile_signing_key::CreateProfileSigningKeyUseCase;
use crate::service::profile_relationship::{
    DeliverProfileActivitiesUseCase, ProfileDeliveryCycleReport, ProfileDeliveryEnv,
};
use crate::service::session_context::OrganizationContext;
use driver::crypto::{Argon2Encryptor, FilePasswordProvider, Rsa2048RawGenerator};
use driver::database::isolated_db::IsolatedDatabase;
use driver::database::PostgresDatabase;
use driver::http_signing::HttpSignerImpl;
use driver::profile_delivery::{
    HttpProfileDeliverySender, HttpRemoteActorResolver, ProfileDeliveryNetworkSettings,
};
use kernel::interfaces::config::{DependOnPublicBaseUrl, PublicBaseUrl};
use kernel::interfaces::crypto::{
    DependOnKeyEncryptor, DependOnPasswordProvider, DependOnRawKeyGenerator, SigningAlgorithm,
};
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::http_signing::DependOnHttpSigner;
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{
    AuthAccountRepository, AuthHostRepository, DependOnAuthAccountRepository,
    DependOnAuthHostRepository, DependOnProfileDeliveryJobRepository,
    DependOnProfileDeliverySender, DependOnRemoteActorResolver, ProfileDeliveryJobRepository,
};
use kernel::prelude::entity::{
    Account, AccountKind, AuthAccountId, AuthHostId, Nanoid, OrgRole, ProfileDeliveryJob,
    ProfileDeliveryJobId, ProfileDeliveryPolicy, ProfileId,
};
use kernel::test_utils::{
    unique_account_name, AccountBuilder, AuthAccountBuilder, AuthHostBuilder, ProfileBuilder,
};
use sqlx::Executor;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub(super) const LOCAL_BASE: &str = "https://local.example";

#[derive(Clone)]
pub(super) struct TestModule {
    database: PostgresDatabase,
    password_provider: Arc<FilePasswordProvider>,
    raw_key_generator: Arc<Rsa2048RawGenerator>,
    key_encryptor: Arc<Argon2Encryptor>,
    http_signer: HttpSignerImpl,
    public_base_url: PublicBaseUrl,
    sender: Arc<HttpProfileDeliverySender>,
    resolver: Arc<HttpRemoteActorResolver>,
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
    type KeyEncryptor = Argon2Encryptor;
    fn key_encryptor(&self) -> &Self::KeyEncryptor {
        self.key_encryptor.as_ref()
    }
}

impl DependOnHttpSigner for TestModule {
    type HttpSigner = HttpSignerImpl;
    fn http_signer(&self) -> &Self::HttpSigner {
        &self.http_signer
    }
}

impl DependOnPublicBaseUrl for TestModule {
    fn public_base_url(&self) -> &PublicBaseUrl {
        &self.public_base_url
    }
}

impl DependOnProfileDeliverySender for TestModule {
    type ProfileDeliverySender = HttpProfileDeliverySender;
    fn profile_delivery_sender(&self) -> &Self::ProfileDeliverySender {
        self.sender.as_ref()
    }
}

impl DependOnRemoteActorResolver for TestModule {
    type RemoteActorResolver = HttpRemoteActorResolver;
    fn remote_actor_resolver(&self) -> &Self::RemoteActorResolver {
        self.resolver.as_ref()
    }
}

pub(super) struct User {
    pub auth: AuthAccountId,
    #[allow(dead_code)]
    pub account: Account,
    pub profile_id: ProfileId,
    pub nanoid: String,
}

pub(super) struct Env {
    pub iso: IsolatedDatabase,
    pub module: TestModule,
    pub peer: MockServer,
    offset_seconds: Arc<AtomicI64>,
    _password: tempfile::NamedTempFile,
}

pub(super) fn round(now: OffsetDateTime) -> OffsetDateTime {
    now.replace_nanosecond(now.nanosecond() / 1000 * 1000)
        .unwrap_or(now)
}

impl Env {
    pub async fn new() -> Self {
        Self::with_sender_settings(ProfileDeliveryNetworkSettings::default()).await
    }

    pub async fn with_sender_settings(settings: ProfileDeliveryNetworkSettings) -> Self {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let password = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(password.path(), b"profile-relationship-password").unwrap();
        let module = TestModule {
            database: iso.database.clone(),
            password_provider: Arc::new(FilePasswordProvider::with_paths(
                password.path(),
                password.path(),
            )),
            raw_key_generator: Arc::new(Rsa2048RawGenerator),
            key_encryptor: Arc::new(Argon2Encryptor::default()),
            http_signer: HttpSignerImpl,
            public_base_url: PublicBaseUrl::new(LOCAL_BASE.to_string()),
            sender: Arc::new(
                HttpProfileDeliverySender::allowing_insecure_loopback().with_settings(settings),
            ),
            resolver: Arc::new(HttpRemoteActorResolver::allowing_insecure_loopback()),
        };
        Self {
            iso,
            module,
            peer: MockServer::start().await,
            offset_seconds: Arc::new(AtomicI64::new(0)),
            _password: password,
        }
    }

    pub fn peer_url(&self, path: &str) -> String {
        format!("{}{path}", self.peer.uri())
    }

    /// peer の actor 文書 (`GET {path}`) を公開する。`id` は自身の URL、inbox は `/inbox`。
    pub async fn mount_actor(&self, actor_path: &str) -> String {
        let id = self.peer_url(actor_path);
        self.mount_actor_as(actor_path, &id).await;
        id
    }

    /// `GET {actor_path}` が `id` として任意の URL を名乗る actor 文書を返す。
    pub async fn mount_actor_as(&self, actor_path: &str, id: &str) {
        Mock::given(method("GET"))
            .and(path(actor_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": id,
                "type": "Person",
                "inbox": self.peer_url("/inbox"),
            })))
            .mount(&self.peer)
            .await;
    }

    /// peer の WebFinger (`name@{peer の host:port}`) が `actor_url` を ActivityPub の self link として返す。
    pub async fn mount_webfinger(&self, name: &str, actor_url: &str) -> String {
        let domain = self.peer.address().to_string();
        let resource = format!("acct:{name}@{domain}");
        Mock::given(method("GET"))
            .and(path("/.well-known/webfinger"))
            .and(query_param("resource", resource.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "subject": resource,
                "links": [{"rel": "self", "type": "application/activity+json", "href": actor_url}],
            })))
            .mount(&self.peer)
            .await;
        format!("acct:{name}@{domain}")
    }

    pub async fn webfinger_requests(&self) -> usize {
        self.peer
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|request| request.url.path() == "/.well-known/webfinger")
            .count()
    }

    /// peer の inbox (`POST /inbox`) の応答を設定する。以前の設定と受信記録は消える。
    pub async fn respond_inbox(&self, template: ResponseTemplate) {
        self.peer.reset().await;
        Mock::given(method("POST"))
            .and(path("/inbox"))
            .respond_with(template)
            .mount(&self.peer)
            .await;
    }

    pub async fn inbox_requests(&self) -> Vec<wiremock::Request> {
        self.peer
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|request| request.method.as_str() == "POST" && request.url.path() == "/inbox")
            .collect()
    }

    /// worker の「現在時刻」を進める (バックオフ・lease 切れの待ち時間を実時間で待たない)。
    pub fn advance_clock(&self, by: Duration) {
        self.offset_seconds
            .fetch_add(by.whole_seconds(), Ordering::SeqCst);
    }

    pub fn now(&self) -> OffsetDateTime {
        round(
            OffsetDateTime::now_utc()
                + Duration::seconds(self.offset_seconds.load(Ordering::SeqCst)),
        )
    }

    pub async fn cycle(&self, worker_id: &str) -> ProfileDeliveryCycleReport {
        self.cycle_with(worker_id, &ProfileDeliveryPolicy::default())
            .await
    }

    /// jitter は常に 1.0 (バックオフは基準値どおり)。
    pub async fn cycle_with(
        &self,
        worker_id: &str,
        policy: &ProfileDeliveryPolicy,
    ) -> ProfileDeliveryCycleReport {
        let clock = || self.now();
        self.cycle_with_clock(worker_id, policy, &clock).await
    }

    /// 任意の時計 (呼び出し回数に応じて飛ぶ時計など) で 1 サイクル回す。
    pub async fn cycle_with_clock(
        &self,
        worker_id: &str,
        policy: &ProfileDeliveryPolicy,
        clock: &(dyn Fn() -> OffsetDateTime + Sync),
    ) -> ProfileDeliveryCycleReport {
        let jitter = || 1.0;
        let stop = || false;
        self.module
            .deliver_profile_activities(
                worker_id,
                policy,
                &ProfileDeliveryEnv {
                    clock,
                    jitter: &jitter,
                    should_stop: &stop,
                },
            )
            .await
            .unwrap()
    }

    async fn seed_account(&self, kind: AccountKind) -> Account {
        let account = AccountBuilder::new()
            .kind(kind)
            .name(unique_account_name())
            .build();
        let mut executor = self.module.database.connection().await.unwrap();
        self.module
            .account_read_model()
            .create(&mut executor, &account)
            .await
            .unwrap();
        account
    }

    pub async fn seed_profile_in(&self, account: &Account) -> (ProfileId, String) {
        let profile = ProfileBuilder::new()
            .id(ProfileId::new(kernel::generate_id()))
            .account_id(account.id().clone())
            .nanoid(Nanoid::default())
            .build();
        let mut executor = self.module.database.connection().await.unwrap();
        self.module
            .profile_read_model()
            .create(&mut executor, &profile)
            .await
            .unwrap();
        (profile.id().clone(), profile.nanoid().as_ref().to_string())
    }

    /// ログイン中の個人 (auth account・個人 Account・その Profile)。
    pub async fn seed_user(&self) -> User {
        let host_id = AuthHostId::default();
        let auth_host = AuthHostBuilder::new()
            .id(host_id.clone())
            .url(format!("https://issuer.example/{}", kernel::generate_id()))
            .build();
        let auth_id = AuthAccountId::default();
        let auth_account = AuthAccountBuilder::new()
            .id(auth_id.clone())
            .host(host_id)
            .client_id(format!("subject-{}", kernel::generate_id()))
            .build();
        let account = self.seed_account(AccountKind::Personal).await;
        {
            let mut executor = self.module.database.connection().await.unwrap();
            self.module
                .auth_host_repository()
                .create(&mut executor, &auth_host)
                .await
                .unwrap();
            self.module
                .auth_account_repository()
                .create(&mut executor, &auth_account)
                .await
                .unwrap();
            self.module
                .account_read_model()
                .link_auth_account(&mut executor, account.id(), &auth_id)
                .await
                .unwrap();
        }
        let (profile_id, nanoid) = self.seed_profile_in(&account).await;
        User {
            auth: auth_id,
            account,
            profile_id,
            nanoid,
        }
    }

    pub async fn seed_organization(&self) -> Account {
        self.seed_account(AccountKind::Organization).await
    }

    pub fn org_context(account: &Account) -> OrganizationContext {
        OrganizationContext {
            org_account_id: account.id().clone(),
            org_account_nanoid: account.nanoid().as_ref().to_string(),
            role: OrgRole::Owner,
        }
    }

    /// Profile の署名鍵を作る (本番コードは鍵を生成しない。P4 が所有)。
    pub async fn create_key(&self, profile_id: &ProfileId, nanoid: &str) -> String {
        let deps = self.module.clone();
        let profile_id = profile_id.clone();
        let nanoid = Nanoid::new(nanoid.to_string());
        let key = self
            .module
            .transaction_manager()
            .transaction(move |executor| {
                Box::pin(async move {
                    deps.create_profile_signing_key(
                        executor,
                        profile_id,
                        &nanoid,
                        SigningAlgorithm::Rsa2048,
                    )
                    .await
                })
            })
            .await
            .unwrap();
        key.public_key_pem.clone()
    }

    pub async fn job(&self, id: &ProfileDeliveryJobId) -> ProfileDeliveryJob {
        let mut executor = self.module.database.connection().await.unwrap();
        self.module
            .profile_delivery_job_repository()
            .find_by_id(&mut executor, id)
            .await
            .unwrap()
            .expect("job exists")
    }

    /// Profile が送信元の全 job (古い順)。
    pub async fn jobs_of(&self, profile_id: &ProfileId) -> Vec<ProfileDeliveryJob> {
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM profile_delivery_jobs WHERE sender_profile_id = $1 ORDER BY id",
        )
        .bind(profile_id.as_ref())
        .fetch_all(&self.iso.pool)
        .await
        .unwrap();
        let mut jobs = Vec::new();
        for id in ids {
            jobs.push(self.job(&ProfileDeliveryJobId::new(id)).await);
        }
        jobs
    }

    pub async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar::<_, i64>(sql)
            .fetch_one(&self.iso.pool)
            .await
            .unwrap()
    }

    pub async fn execute(&self, sql: &str) {
        self.iso.pool.execute(sql).await.unwrap();
    }
}
