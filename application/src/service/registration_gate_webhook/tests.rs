//! 送信先管理 use case のテスト。実 DB (`DATABASE_URL`) + ローカル fake receiver (wiremock) で、
//! 認可・CRUD・マスク・暗号化・登録時の宛先検査・テスト送信・再試行・failed 一覧・環境変数の取り込みを
//! 固定する。`test_with::env(DATABASE_URL)` で未設定時は skip されるが、CI は PostgreSQL service 付きで
//! 必ず実行する (skip のみの成功は不可)。

use super::*;
use driver::database::PostgresDatabase;
use driver::webhook::{AeadWebhookSecretCipher, HostResolver, HttpWebhookSender};
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::permission::{
    DependOnPermissionChecker, InstanceRole, PermissionChecker, PermissionReq,
};
use kernel::interfaces::repository::DependOnWebhookDeliveryRepository;
use kernel::prelude::entity::{
    WebhookEndpointHealth, WebhookEndpointSource, WebhookFailureReason, WebhookResultClass,
};
use sqlx::PgConnection;
use std::net::SocketAddr;
use std::sync::Arc;
use time::Duration;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const ADMIN: i64 = 1;
const OUTSIDER: i64 = 2;
const SECRET_A: &str = "whsec_application_secret_AAAAAAAAAAAA";
const SECRET_B: &str = "whsec_application_secret_BBBBBBBBBBBB";
const KEY_1: [u8; 32] = [11u8; 32];
const KEY_2: [u8; 32] = [22u8; 32];

#[derive(Clone)]
struct FakePermissionChecker {
    admins: Vec<AuthAccountId>,
}

impl PermissionChecker for FakePermissionChecker {
    async fn check(
        &self,
        subject: &AuthAccountId,
        req: &PermissionReq,
    ) -> error_stack::Result<bool, KernelError> {
        assert_eq!(req.namespace(), "Instance");
        assert_eq!(req.permission_name(), "administrate");
        Ok(self.admins.contains(subject))
    }

    async fn list_instance_roles(
        &self,
        _subject: &AuthAccountId,
    ) -> error_stack::Result<Vec<InstanceRole>, KernelError> {
        Ok(Vec::new())
    }
}

struct Module {
    db: PostgresDatabase,
    sender: HttpWebhookSender,
    cipher: AeadWebhookSecretCipher,
    permission_checker: FakePermissionChecker,
}

impl DependOnDatabaseConnection for Module {
    type DatabaseConnection = PostgresDatabase;

    fn database_connection(&self) -> &Self::DatabaseConnection {
        &self.db
    }
}

impl DependOnWebhookDeliveryRepository for Module {
    type WebhookDeliveryRepository =
        <PostgresDatabase as DependOnWebhookDeliveryRepository>::WebhookDeliveryRepository;

    fn webhook_delivery_repository(&self) -> &Self::WebhookDeliveryRepository {
        self.db.webhook_delivery_repository()
    }
}

impl DependOnWebhookSender for Module {
    type WebhookSender = HttpWebhookSender;

    fn webhook_sender(&self) -> &Self::WebhookSender {
        &self.sender
    }
}

impl DependOnWebhookSecretCipher for Module {
    type WebhookSecretCipher = AeadWebhookSecretCipher;

    fn webhook_secret_cipher(&self) -> &Self::WebhookSecretCipher {
        &self.cipher
    }
}

impl DependOnPermissionChecker for Module {
    type PermissionChecker = FakePermissionChecker;

    fn permission_checker(&self) -> &Self::PermissionChecker {
        &self.permission_checker
    }
}

struct Fixture {
    module: Module,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    async fn with(sender: HttpWebhookSender, cipher: AeadWebhookSecretCipher) -> Self {
        let guard = DB_LOCK.lock().await;
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.expect("DATABASE_URL set");
        let mut conn = db.connection().await.unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query(
            "TRUNCATE registration_gate_webhook_attempts, registration_gate_webhook_deliveries, \
             registration_gate_webhook_endpoints, registration_gate_webhook_endpoint_audit, \
             registration_gate_webhook_env_import, registration_gate_webhook_state",
        )
        .execute(con)
        .await
        .unwrap();
        drop(conn);
        Self {
            module: Module {
                db,
                sender,
                cipher,
                permission_checker: FakePermissionChecker {
                    admins: vec![AuthAccountId::new(ADMIN)],
                },
            },
            _guard: guard,
        }
    }

    /// 登録時・接続時に loopback の http を許す test 用 sender + 鍵あり。
    async fn new() -> Self {
        Self::with(
            loopback_sender(),
            AeadWebhookSecretCipher::from_key_bytes(KEY_1),
        )
        .await
    }

    fn admin(&self) -> AuthAccountId {
        AuthAccountId::new(ADMIN)
    }

    fn outsider(&self) -> AuthAccountId {
        AuthAccountId::new(OUTSIDER)
    }

    async fn count(&self, table: &str) -> i64 {
        let mut conn = self.module.db.connection().await.unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(con)
            .await
            .unwrap()
    }

    async fn audit(&self) -> Vec<(String, String, Option<String>)> {
        let mut conn = self.module.db.connection().await.unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query_as(
            "SELECT operation, outcome, detail \
             FROM registration_gate_webhook_endpoint_audit ORDER BY id",
        )
        .fetch_all(con)
        .await
        .unwrap()
    }

    async fn insert_failed(&self, endpoint_id: i64, event_id: i64, failed_at: OffsetDateTime) {
        let mut conn = self.module.db.connection().await.unwrap();
        let con: &mut PgConnection = &mut conn;
        sqlx::query(
            r#"
            INSERT INTO registration_gate_webhook_deliveries (
                endpoint_id, event_id, kind, payload, status, attempt_count, cycle_attempt_count,
                cycle_started_at, next_attempt_at, last_result, last_http_status, failure_reason,
                failed_at, created_at, updated_at
            ) VALUES ($1, $2, 'enforcement_lost_stopped', '{}', 'failed', 8, 8, $3, $3,
                      'http_server_error', 500, 'retries_exhausted', $4, $3, $3)
            "#,
        )
        .bind(endpoint_id)
        .bind(event_id)
        .bind(failed_at - Duration::hours(1))
        .bind(failed_at)
        .execute(con)
        .await
        .unwrap();
    }
}

fn loopback_sender() -> HttpWebhookSender {
    HttpWebhookSender::disabled_allowing_insecure_loopback_destinations()
}

struct FixedResolver(Result<Vec<SocketAddr>, ()>);

impl HostResolver for FixedResolver {
    fn resolve<'a>(
        &'a self,
        _host: &'a str,
        port: u16,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + 'a>,
    > {
        let answer = self.0.clone();
        Box::pin(async move {
            answer
                .map(|addresses| {
                    addresses
                        .into_iter()
                        .map(|address| SocketAddr::new(address.ip(), port))
                        .collect()
                })
                .map_err(|_| std::io::Error::other("no such host"))
        })
    }
}

fn strict_sender(answer: Result<Vec<SocketAddr>, ()>) -> HttpWebhookSender {
    HttpWebhookSender::disabled().with_resolver(Arc::new(FixedResolver(answer)))
}

fn public_address() -> SocketAddr {
    "93.184.216.34:443".parse().unwrap()
}

fn create_input(
    url: &str,
    secret: &str,
    categories: &[WebhookCategory],
) -> CreateWebhookEndpointInput {
    CreateWebhookEndpointInput {
        url: url.to_string(),
        secret: WriteOnlySecret::new(secret),
        categories: categories.to_vec(),
    }
}

fn update_input(revision: i64) -> UpdateWebhookEndpointInput {
    UpdateWebhookEndpointInput {
        revision,
        url: None,
        secret: None,
        categories: None,
    }
}

fn rejection_of<T: std::fmt::Debug>(
    result: error_stack::Result<T, KernelError>,
) -> WebhookEndpointRejection {
    let report = result.expect_err("expected a rejection");
    assert_eq!(report.current_context(), &KernelError::Validation);
    *report
        .downcast_ref::<WebhookEndpointRejection>()
        .expect("a rejection is attached")
}

fn kind_of<T: std::fmt::Debug>(result: error_stack::Result<T, KernelError>) -> KernelError {
    match result.expect_err("expected an error").current_context() {
        KernelError::PermissionDenied => KernelError::PermissionDenied,
        KernelError::NotFound => KernelError::NotFound,
        KernelError::Concurrency => KernelError::Concurrency,
        KernelError::Validation => KernelError::Validation,
        other => panic!("unexpected error {other:?}"),
    }
}

async fn mount_status(server: &MockServer, status: u16) {
    Mock::given(any())
        .respond_with(ResponseTemplate::new(status))
        .mount(server)
        .await;
}

async fn request_count(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().len()
}

async fn create(fx: &Fixture, url: &str, secret: &str) -> WebhookEndpointView {
    fx.module
        .create_registration_gate_webhook_endpoint(
            &fx.admin(),
            create_input(url, secret, &WebhookCategory::ALL),
        )
        .await
        .unwrap()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn every_admin_use_case_rejects_non_admins_before_touching_anything() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 204).await;
    let outsider = fx.outsider();
    let url = format!("{}/hook", server.uri());
    let existing = create(&fx, &url, SECRET_A).await;
    let audit_before = fx.audit().await;
    let id = existing.id;

    let m = &fx.module;
    assert_eq!(
        kind_of(m.list_registration_gate_webhook_endpoints(&outsider).await),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind_of(
            m.get_registration_gate_webhook_endpoint(&outsider, id)
                .await
        ),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind_of(
            m.create_registration_gate_webhook_endpoint(
                &outsider,
                create_input(
                    "https://other.example.net/h",
                    SECRET_B,
                    &WebhookCategory::ALL
                ),
            )
            .await
        ),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind_of(
            m.update_registration_gate_webhook_endpoint(&outsider, id, update_input(1))
                .await
        ),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind_of(
            m.delete_registration_gate_webhook_endpoint(&outsider, id, 1)
                .await
        ),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind_of(m.send_registration_gate_webhook_test(&outsider, id).await),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind_of(
            m.retry_failed_registration_gate_webhooks(&outsider, None, None)
                .await
        ),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind_of(
            m.list_failed_registration_gate_webhook_deliveries(&outsider, None, None, None)
                .await
        ),
        KernelError::PermissionDenied
    );
    assert_eq!(
        request_count(&server).await,
        0,
        "no request leaves for a non-admin"
    );
    assert_eq!(fx.count("registration_gate_webhook_endpoints").await, 1);
    assert_eq!(
        fx.audit().await,
        audit_before,
        "denied calls are not audited as operations"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn created_endpoints_are_shown_only_as_masked_urls_and_never_expose_the_secret() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    mount_status(&server, 204).await;
    let host = server.address().to_string();
    let url = format!(
        "{}/services/T000/B000/abcdefgh12a8f2?token=querysecret",
        server.uri()
    );

    let view = create(&fx, &url, SECRET_A).await;
    assert_eq!(view.masked_url, format!("{host}/••••/a8f2"));
    assert_eq!(view.revision, Some(1));
    assert_eq!(view.source, WebhookEndpointSource::Api);
    assert_eq!(view.health, WebhookEndpointHealth::Healthy);
    assert!(view.secret_configured);
    assert_eq!(view.categories, WebhookCategories::all());

    let list = fx
        .module
        .list_registration_gate_webhook_endpoints(&fx.admin())
        .await
        .unwrap();
    assert_eq!(list.endpoints, vec![view.clone()]);
    assert!(list.encryption_key_configured);
    assert!(!list.has_failed_deliveries);
    assert_eq!(list.max_endpoints, MAX_WEBHOOK_ENDPOINTS);
    let fetched = fx
        .module
        .get_registration_gate_webhook_endpoint(&fx.admin(), view.id)
        .await
        .unwrap();
    assert_eq!(fetched, view);

    let rendered = format!("{view:?} {list:?}");
    for leaked in [
        SECRET_A,
        "querysecret",
        "services/T000",
        "abcdefgh",
        "token=",
    ] {
        assert!(!rendered.contains(leaked), "view leaks {leaked}");
    }
    assert_eq!(
        fx.audit().await,
        vec![("create".to_string(), "succeeded".to_string(), None)]
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn short_path_tokens_are_fully_hidden_in_the_masked_url() {
    let fx = Fixture::new().await;
    let server = MockServer::start().await;
    let host = server.address().to_string();
    let view = create(&fx, &format!("{}/short-token", server.uri()), SECRET_A).await;
    assert_eq!(view.masked_url, format!("{host}/••••"));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn invalid_registrations_are_rejected_with_fixed_codes_and_nothing_is_stored() {
    let admin = AuthAccountId::new(ADMIN);
    let ok_url = "https://hooks.example.net/ok";
    let mut cases: Vec<(CreateWebhookEndpointInput, WebhookEndpointRejection)> = Vec::new();
    cases.push((
        create_input(ok_url, SECRET_A, &[]),
        WebhookEndpointRejection::NoCategories,
    ));
    cases.push((
        create_input(ok_url, "short", &WebhookCategory::ALL),
        WebhookEndpointRejection::SecretTooShort,
    ));
    cases.push((
        create_input(ok_url, &"s".repeat(257), &WebhookCategory::ALL),
        WebhookEndpointRejection::SecretTooLong,
    ));
    for (url, expected) in [
        ("not a url", WebhookEndpointRejection::InvalidUrl),
        (
            "http://hooks.example.net/h",
            WebhookEndpointRejection::UnsupportedScheme,
        ),
        (
            "https://user:pass@hooks.example.net/h",
            WebhookEndpointRejection::CredentialsInUrl,
        ),
        (
            "https://hooks.example.net/h#frag",
            WebhookEndpointRejection::FragmentInUrl,
        ),
        (
            "https://hooks.example.net/h#",
            WebhookEndpointRejection::FragmentInUrl,
        ),
    ] {
        cases.push((create_input(url, SECRET_A, &WebhookCategory::ALL), expected));
    }
    let too_long = format!("https://hooks.example.net/{}", "a".repeat(2100));
    cases.push((
        create_input(&too_long, SECRET_A, &WebhookCategory::ALL),
        WebhookEndpointRejection::UrlTooLong,
    ));
    // 使うのは本番と同じ厳格な宛先検査 (loopback 例外なし)。
    let strict = Fixture::with(
        strict_sender(Ok(vec![public_address()])),
        AeadWebhookSecretCipher::from_key_bytes(KEY_1),
    )
    .await;
    for (input, expected) in cases {
        let url = input.url.clone();
        let result = strict
            .module
            .create_registration_gate_webhook_endpoint(&admin, input)
            .await;
        assert_eq!(rejection_of(result), expected, "{url:.60}");
    }
    assert_eq!(strict.count("registration_gate_webhook_endpoints").await, 0);
    let audit = strict.audit().await;
    assert!(audit
        .iter()
        .all(|(operation, outcome, detail)| operation == "create"
            && outcome == "rejected"
            && detail.is_some()));
    for (_, _, detail) in &audit {
        let detail = detail.as_deref().unwrap();
        assert!(
            !detail.contains("example") && !detail.contains("pass"),
            "{detail}"
        );
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn internal_destinations_are_rejected_at_registration_in_every_notation() {
    let fx = Fixture::with(
        strict_sender(Ok(vec![public_address()])),
        AeadWebhookSecretCipher::from_key_bytes(KEY_1),
    )
    .await;
    let admin = fx.admin();
    for url in [
        "https://localhost/hook",
        "https://LOCALHOST./hook",
        "https://api.localhost/hook",
        "https://127.0.0.1/hook",
        "https://127.1/hook",
        "https://0x7f.0.0.1/hook",
        "https://2130706433/hook",
        "https://017700000001/hook",
        "https://0.0.0.0/hook",
        "https://10.0.0.5/hook",
        "https://192.168.1.1/hook",
        "https://172.16.0.1/hook",
        "https://100.64.0.1/hook",
        "https://169.254.169.254/latest/meta-data/",
        "https://metadata.google.internal/hook",
        "https://[::1]/hook",
        "https://[::ffff:127.0.0.1]/hook",
        "https://[::ffff:7f00:1]/hook",
        "https://[::ffff:169.254.169.254]/hook",
        "https://[fe80::1]/hook",
        "https://[fc00::1]/hook",
        "https://[64:ff9b::7f00:1]/hook",
    ] {
        let result = fx
            .module
            .create_registration_gate_webhook_endpoint(
                &admin,
                create_input(url, SECRET_A, &WebhookCategory::ALL),
            )
            .await;
        assert_eq!(
            rejection_of(result),
            WebhookEndpointRejection::ForbiddenDestination,
            "{url}"
        );
    }
    // IPv6 ゾーン付きは URL として解釈できず拒否。
    let result = fx
        .module
        .create_registration_gate_webhook_endpoint(
            &admin,
            create_input(
                "https://[fe80::1%25eth0]/hook",
                SECRET_A,
                &WebhookCategory::ALL,
            ),
        )
        .await;
    assert_eq!(rejection_of(result), WebhookEndpointRejection::InvalidUrl);
    assert_eq!(fx.count("registration_gate_webhook_endpoints").await, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn hostnames_are_checked_against_every_resolved_address() {
    let admin = AuthAccountId::new(ADMIN);
    let internal: SocketAddr = "10.1.2.3:443".parse().unwrap();
    let private_v6: SocketAddr = "[fd00::5]:443".parse().unwrap();
    let mapped: SocketAddr = "[::ffff:10.0.0.1]:443".parse().unwrap();
    for (answer, expected) in [
        (
            Ok(vec![internal]),
            WebhookEndpointRejection::ForbiddenDestination,
        ),
        (
            Ok(vec![public_address(), internal]),
            WebhookEndpointRejection::ForbiddenDestination,
        ),
        (
            Ok(vec![public_address(), private_v6]),
            WebhookEndpointRejection::ForbiddenDestination,
        ),
        (
            Ok(vec![mapped]),
            WebhookEndpointRejection::ForbiddenDestination,
        ),
        (Ok(vec![]), WebhookEndpointRejection::HostUnresolvable),
        (Err(()), WebhookEndpointRejection::HostUnresolvable),
    ] {
        let fx = Fixture::with(
            strict_sender(answer),
            AeadWebhookSecretCipher::from_key_bytes(KEY_1),
        )
        .await;
        let result = fx
            .module
            .create_registration_gate_webhook_endpoint(
                &admin,
                create_input(
                    "https://rebind.example.net/hook",
                    SECRET_A,
                    &WebhookCategory::ALL,
                ),
            )
            .await;
        assert_eq!(rejection_of(result), expected);
        // 拒否理由は固定語彙だけで、どの内部アドレスに解決したかは含まない。
        assert!(!format!("{expected}").contains("10."));
        assert_eq!(fx.count("registration_gate_webhook_endpoints").await, 0);
    }
    // 全アドレスが公開なら通り、URL は正規化される (host の小文字化・末尾ドット除去・既定ポート省略)。
    let fx = Fixture::with(
        strict_sender(Ok(vec![public_address()])),
        AeadWebhookSecretCipher::from_key_bytes(KEY_1),
    )
    .await;
    let view = fx
        .module
        .create_registration_gate_webhook_endpoint(
            &admin,
            create_input(
                "https://HOOKS.Example.net.:443/hook/abcdefghijkl",
                SECRET_A,
                &WebhookCategory::ALL,
            ),
        )
        .await
        .unwrap();
    assert_eq!(view.masked_url, "hooks.example.net/••••/ijkl");
    let mut conn = fx.module.db.connection().await.unwrap();
    let con: &mut PgConnection = &mut conn;
    let stored: String = sqlx::query_scalar("SELECT url FROM registration_gate_webhook_endpoints")
        .fetch_one(con)
        .await
        .unwrap();
    assert_eq!(stored, "https://hooks.example.net/hook/abcdefghijkl");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn the_equivalent_normalized_url_cannot_be_registered_twice() {
    let fx = Fixture::with(
        strict_sender(Ok(vec![public_address()])),
        AeadWebhookSecretCipher::from_key_bytes(KEY_1),
    )
    .await;
    let admin = fx.admin();
    create(&fx, "https://hooks.example.net/hook", SECRET_A).await;
    let result = fx
        .module
        .create_registration_gate_webhook_endpoint(
            &admin,
            create_input(
                "https://HOOKS.example.net.:443/hook",
                SECRET_B,
                &WebhookCategory::ALL,
            ),
        )
        .await;
    assert_eq!(rejection_of(result), WebhookEndpointRejection::DuplicateUrl);
    assert_eq!(fx.count("registration_gate_webhook_endpoints").await, 1);
    let audit = fx.audit().await;
    assert_eq!(
        audit.last().unwrap(),
        &(
            "create".to_string(),
            "rejected".to_string(),
            Some("duplicate_url".to_string())
        )
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn the_endpoint_limit_is_explicit_and_documented() {
    let fx = Fixture::new().await;
    let admin = fx.admin();
    for index in 0..MAX_WEBHOOK_ENDPOINTS {
        create(
            &fx,
            &format!("http://127.0.0.1:{}/hook", 20000 + index),
            SECRET_A,
        )
        .await;
    }
    let result = fx
        .module
        .create_registration_gate_webhook_endpoint(
            &admin,
            create_input(
                "http://127.0.0.1:29999/hook",
                SECRET_A,
                &WebhookCategory::ALL,
            ),
        )
        .await;
    assert_eq!(
        rejection_of(result),
        WebhookEndpointRejection::EndpointLimitReached
    );
    assert_eq!(
        fx.count("registration_gate_webhook_endpoints").await,
        MAX_WEBHOOK_ENDPOINTS
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn without_an_encryption_key_nothing_is_created_and_secrets_cannot_be_replaced() {
    let fx = Fixture::new().await;
    let admin = fx.admin();
    let created = create(&fx, "http://127.0.0.1:20002/hook", SECRET_A).await;
    // 鍵なしで起動した状態 (同じ DB)。
    let keyless = Module {
        db: PostgresDatabase::new().await.unwrap(),
        sender: loopback_sender(),
        cipher: AeadWebhookSecretCipher::not_configured(),
        permission_checker: fx.module.permission_checker.clone(),
    };
    let result = keyless
        .create_registration_gate_webhook_endpoint(
            &admin,
            create_input(
                "http://127.0.0.1:20001/hook",
                SECRET_A,
                &WebhookCategory::ALL,
            ),
        )
        .await;
    assert_eq!(
        rejection_of(result),
        WebhookEndpointRejection::EncryptionKeyNotConfigured
    );
    assert_eq!(fx.count("registration_gate_webhook_endpoints").await, 1);
    let list = keyless
        .list_registration_gate_webhook_endpoints(&admin)
        .await
        .unwrap();
    assert!(!list.encryption_key_configured);

    // secret の更新も拒否 (平文で保存しない)。
    let mut input = update_input(1);
    input.secret = Some(WriteOnlySecret::new(SECRET_B));
    let result = keyless
        .update_registration_gate_webhook_endpoint(&admin, created.id, input)
        .await;
    assert_eq!(
        rejection_of(result),
        WebhookEndpointRejection::EncryptionKeyNotConfigured
    );
    // カテゴリだけの更新は鍵なしでもできる。保存済み secret は復号できないので状態は failing。
    let mut input = update_input(1);
    input.categories = Some(vec![WebhookCategory::Override]);
    let updated = keyless
        .update_registration_gate_webhook_endpoint(&admin, created.id, input)
        .await
        .unwrap();
    assert_eq!(updated.revision, Some(2));
    assert_eq!(updated.health, WebhookEndpointHealth::Failing);
    assert_eq!(
        updated.failing_reasons,
        vec![WebhookFailingReason::SecretUndecryptable]
    );
    let audit = fx.audit().await;
    assert!(audit
        .iter()
        .any(|(operation, outcome, detail)| operation == "create"
            && outcome == "rejected"
            && detail.as_deref() == Some("encryption_key_not_configured")));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn update_uses_optimistic_locking_keeps_omitted_fields_and_bumps_the_revision() {
    let fx = Fixture::new().await;
    let admin = fx.admin();
    let created = create(
        &fx,
        "http://127.0.0.1:20010/hook-aaaaaaaaaaaaaaaa",
        SECRET_A,
    )
    .await;
    let id = created.id;

    let mut stale = update_input(7);
    stale.categories = Some(vec![WebhookCategory::Recovery]);
    assert_eq!(
        kind_of(
            fx.module
                .update_registration_gate_webhook_endpoint(&admin, id, stale)
                .await
        ),
        KernelError::Concurrency
    );
    assert_eq!(
        rejection_of(
            fx.module
                .update_registration_gate_webhook_endpoint(&admin, id, update_input(1))
                .await
        ),
        WebhookEndpointRejection::NothingToUpdate
    );
    let mut empty = update_input(1);
    empty.categories = Some(vec![]);
    assert_eq!(
        rejection_of(
            fx.module
                .update_registration_gate_webhook_endpoint(&admin, id, empty)
                .await
        ),
        WebhookEndpointRejection::NoCategories
    );

    let mut change_categories = update_input(1);
    change_categories.categories = Some(vec![WebhookCategory::Recovery, WebhookCategory::AutoStop]);
    let updated = fx
        .module
        .update_registration_gate_webhook_endpoint(&admin, id, change_categories)
        .await
        .unwrap();
    assert_eq!(updated.revision, Some(2));
    assert_eq!(
        updated.masked_url, created.masked_url,
        "URL omitted = unchanged"
    );
    assert_eq!(
        updated.categories.as_slice(),
        [WebhookCategory::Recovery, WebhookCategory::AutoStop]
    );

    let mut replace_url = update_input(2);
    replace_url.url = Some("http://127.0.0.1:20011/new-token-bbbbbbbbbbbb".to_string());
    let updated = fx
        .module
        .update_registration_gate_webhook_endpoint(&admin, id, replace_url)
        .await
        .unwrap();
    assert_eq!(updated.revision, Some(3));
    assert_eq!(updated.masked_url, "127.0.0.1:20011/••••/bbbb");

    let mut replace_secret = update_input(3);
    replace_secret.secret = Some(WriteOnlySecret::new(SECRET_B));
    let updated = fx
        .module
        .update_registration_gate_webhook_endpoint(&admin, id, replace_secret)
        .await
        .unwrap();
    assert_eq!(updated.revision, Some(4));
    assert_eq!(updated.health, WebhookEndpointHealth::Healthy);

    // 重複 URL への更新は拒否され、revision は動かない。
    let other = create(&fx, "http://127.0.0.1:20012/other", SECRET_A).await;
    let mut clash = update_input(1);
    clash.url = Some("http://127.0.0.1:20011/new-token-bbbbbbbbbbbb".to_string());
    assert_eq!(
        rejection_of(
            fx.module
                .update_registration_gate_webhook_endpoint(&admin, other.id, clash)
                .await
        ),
        WebhookEndpointRejection::DuplicateUrl
    );
    assert_eq!(
        fx.module
            .get_registration_gate_webhook_endpoint(&admin, other.id)
            .await
            .unwrap()
            .revision,
        Some(1)
    );
    let mut missing = update_input(1);
    missing.categories = Some(vec![WebhookCategory::Outage]);
    assert_eq!(
        kind_of(
            fx.module
                .update_registration_gate_webhook_endpoint(
                    &admin,
                    WebhookEndpointId::new(42),
                    missing
                )
                .await
        ),
        KernelError::NotFound
    );
    let outcomes: Vec<_> = fx
        .audit()
        .await
        .into_iter()
        .filter(|(operation, _, _)| operation == "update")
        .map(|(_, outcome, _)| outcome)
        .collect();
    assert!(outcomes.contains(&"revision_conflict".to_string()));
    assert!(outcomes.contains(&"succeeded".to_string()));
    assert!(outcomes.contains(&"rejected".to_string()));
    assert!(outcomes.contains(&"not_found".to_string()));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn delete_uses_optimistic_locking_and_removes_the_endpoint() {
    let fx = Fixture::new().await;
    let admin = fx.admin();
    let created = create(&fx, "http://127.0.0.1:20020/hook", SECRET_A).await;
    fx.insert_failed(created.id.as_i64(), 100, now()).await;
    assert_eq!(
        kind_of(
            fx.module
                .delete_registration_gate_webhook_endpoint(&admin, created.id, 9)
                .await
        ),
        KernelError::Concurrency
    );
    fx.module
        .delete_registration_gate_webhook_endpoint(&admin, created.id, 1)
        .await
        .unwrap();
    assert_eq!(
        kind_of(
            fx.module
                .get_registration_gate_webhook_endpoint(&admin, created.id)
                .await
        ),
        KernelError::NotFound
    );
    assert_eq!(
        kind_of(
            fx.module
                .delete_registration_gate_webhook_endpoint(&admin, created.id, 1)
                .await
        ),
        KernelError::NotFound
    );
    assert_eq!(fx.count("registration_gate_webhook_deliveries").await, 0);
    let operations: Vec<_> = fx
        .audit()
        .await
        .into_iter()
        .map(|row| (row.0, row.1))
        .collect();
    assert_eq!(
        operations,
        [
            ("create", "succeeded"),
            ("delete", "revision_conflict"),
            ("delete", "succeeded"),
            ("delete", "not_found"),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn test_send_uses_the_endpoint_secret_and_leaves_no_trace_in_records_or_status() {
    let fx = Fixture::new().await;
    let admin = fx.admin();
    let server = MockServer::start().await;
    mount_status(&server, 204).await;
    let created = create(
        &fx,
        &format!("{}/hook-abcdefghijkl", server.uri()),
        SECRET_A,
    )
    .await;
    let events_before = fx.count("registration_gate_events").await;
    let gate_before = fx.count("registration_gate_state").await;

    let outcome = fx
        .module
        .send_registration_gate_webhook_test(&admin, created.id)
        .await
        .unwrap();
    let WebhookTestOutcome::Sent {
        event_id,
        delivered,
        result,
        http_status,
        ..
    } = outcome
    else {
        panic!("expected a sent test")
    };
    assert!(delivered);
    assert_eq!(result, WebhookResultClass::Delivered);
    assert_eq!(http_status, Some(204));
    assert!(event_id.starts_with("test_"));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "exactly one attempt");
    let header = |name: &str| {
        requests[0]
            .headers
            .get(name)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(header("x-emumet-event-type"), "webhook_test");
    assert_eq!(header("x-emumet-event-id"), event_id);
    driver::webhook::signature::verify(
        SECRET_A.as_bytes(),
        &header("x-emumet-signature"),
        header("x-emumet-timestamp").parse().unwrap(),
        &requests[0].body,
        now().unix_timestamp(),
        600,
    )
    .expect("signed with the endpoint's own secret");

    // 不達でも結果が返るだけ。再試行せず、配送記録・状態・ゲート・通知は変わらない。
    server.reset().await;
    mount_status(&server, 503).await;
    let WebhookTestOutcome::Sent {
        delivered, result, ..
    } = fx
        .module
        .send_registration_gate_webhook_test(&admin, created.id)
        .await
        .unwrap()
    else {
        panic!("expected a sent test")
    };
    assert!(!delivered);
    assert_eq!(result, WebhookResultClass::HttpServerError);
    assert_eq!(request_count(&server).await, 1, "no retry");
    assert_eq!(fx.count("registration_gate_webhook_deliveries").await, 0);
    assert_eq!(fx.count("registration_gate_webhook_attempts").await, 0);
    assert_eq!(fx.count("registration_gate_events").await, events_before);
    assert_eq!(fx.count("registration_gate_state").await, gate_before);
    let view = fx
        .module
        .get_registration_gate_webhook_endpoint(&admin, created.id)
        .await
        .unwrap();
    assert_eq!(
        view.health,
        WebhookEndpointHealth::Healthy,
        "a failed test never flips the chip"
    );
    assert_eq!(view.failed_count, 0);
    assert_eq!(view.revision, Some(1));
    let tests: Vec<_> = fx
        .audit()
        .await
        .into_iter()
        .filter(|(operation, _, _)| operation == "test")
        .collect();
    assert_eq!(
        tests,
        vec![
            ("test".to_string(), "succeeded".to_string(), None),
            (
                "test".to_string(),
                "delivery_failed".to_string(),
                Some("http_server_error".to_string())
            ),
        ]
    );
    assert_eq!(
        kind_of(
            fx.module
                .send_registration_gate_webhook_test(&admin, WebhookEndpointId::new(77))
                .await
        ),
        KernelError::NotFound
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn test_send_to_an_endpoint_with_an_undecryptable_secret_sends_nothing() {
    let fx = Fixture::new().await;
    let admin = fx.admin();
    let server = MockServer::start().await;
    mount_status(&server, 204).await;
    let created = create(&fx, &format!("{}/hook", server.uri()), SECRET_A).await;
    let other_key = Module {
        db: PostgresDatabase::new().await.unwrap(),
        sender: loopback_sender(),
        cipher: AeadWebhookSecretCipher::from_key_bytes(KEY_2),
        permission_checker: fx.module.permission_checker.clone(),
    };
    let WebhookTestOutcome::Sent {
        delivered, result, ..
    } = other_key
        .send_registration_gate_webhook_test(&admin, created.id)
        .await
        .unwrap()
    else {
        panic!("expected a sent outcome")
    };
    assert!(!delivered);
    assert_eq!(result, WebhookResultClass::SecretUndecryptable);
    assert_eq!(request_count(&server).await, 0);
    let view = other_key
        .get_registration_gate_webhook_endpoint(&admin, created.id)
        .await
        .unwrap();
    assert_eq!(view.health, WebhookEndpointHealth::Failing);
    assert_eq!(
        view.failing_reasons,
        vec![WebhookFailingReason::SecretUndecryptable]
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn status_chip_is_failing_while_failed_deliveries_exist_and_healthy_after_retry() {
    let fx = Fixture::new().await;
    let admin = fx.admin();
    let a = create(&fx, "http://127.0.0.1:20030/a", SECRET_A).await;
    let b = create(&fx, "http://127.0.0.1:20031/b", SECRET_B).await;
    fx.insert_failed(a.id.as_i64(), 11, now()).await;
    fx.insert_failed(a.id.as_i64(), 12, now()).await;

    let list = fx
        .module
        .list_registration_gate_webhook_endpoints(&admin)
        .await
        .unwrap();
    assert!(list.has_failed_deliveries);
    let a_view = list.endpoints.iter().find(|view| view.id == a.id).unwrap();
    let b_view = list.endpoints.iter().find(|view| view.id == b.id).unwrap();
    assert_eq!(a_view.health, WebhookEndpointHealth::Failing);
    assert_eq!(
        a_view.failing_reasons,
        vec![WebhookFailingReason::FailedDeliveries]
    );
    assert_eq!(a_view.failed_count, 2);
    assert_eq!(a_view.summary.counts.failed, 2);
    assert_eq!(b_view.health, WebhookEndpointHealth::Healthy);
    assert_eq!(b_view.failed_count, 0);

    let requeued = fx
        .module
        .retry_failed_registration_gate_webhooks(&admin, Some(a.id), None)
        .await
        .unwrap();
    assert_eq!(requeued, 2);
    let list = fx
        .module
        .list_registration_gate_webhook_endpoints(&admin)
        .await
        .unwrap();
    assert!(!list.has_failed_deliveries);
    assert!(list
        .endpoints
        .iter()
        .all(|view| view.health == WebhookEndpointHealth::Healthy));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn failed_deliveries_page_by_cursor_past_fifty_and_each_can_be_retried_alone() {
    let fx = Fixture::new().await;
    let admin = fx.admin();
    let a = create(&fx, "http://127.0.0.1:20040/a", SECRET_A).await;
    let b = create(&fx, "http://127.0.0.1:20041/b", SECRET_B).await;
    let t0 = now();
    for index in 0..60i64 {
        fx.insert_failed(a.id.as_i64(), 3000 + index, t0 + Duration::seconds(index))
            .await;
    }
    fx.insert_failed(b.id.as_i64(), 3000, t0 + Duration::seconds(5))
        .await;
    let list = fx
        .module
        .list_registration_gate_webhook_endpoints(&admin)
        .await
        .unwrap();
    assert!(list.has_failed_deliveries);

    // 既定ページ (50 件) の次ページに、最新 50 件の外の failed がある。
    let first = fx
        .module
        .list_failed_registration_gate_webhook_deliveries(&admin, Some(a.id), None, None)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 50);
    let cursor = WebhookFailureCursor::decode(first.next_cursor.as_deref().unwrap()).unwrap();
    let second = fx
        .module
        .list_failed_registration_gate_webhook_deliveries(&admin, Some(a.id), Some(cursor), None)
        .await
        .unwrap();
    assert_eq!(second.items.len(), 10);
    assert!(second.next_cursor.is_none());
    let oldest = second.items.last().unwrap().clone();
    assert_eq!(*oldest.event_id.as_ref(), 3000);
    assert_eq!(oldest.endpoint_id, a.id);
    assert_eq!(oldest.category(), WebhookCategory::AutoStop);
    assert_eq!(
        oldest.last_result,
        Some(WebhookResultClass::HttpServerError)
    );
    assert_eq!(
        oldest.failure_reason,
        Some(WebhookFailureReason::RetriesExhausted)
    );

    // limit は 1..=100 に丸められる。
    let page = fx
        .module
        .list_failed_registration_gate_webhook_deliveries(&admin, None, None, Some(1000))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 61);
    let page = fx
        .module
        .list_failed_registration_gate_webhook_deliveries(&admin, None, None, Some(0))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);

    // その 1 件 (A の 3000。B にも同じ event ID がある) だけを再試行できる。
    let requeued = fx
        .module
        .retry_failed_registration_gate_webhooks(&admin, Some(a.id), Some(oldest.event_id.clone()))
        .await
        .unwrap();
    assert_eq!(requeued, 1);
    assert_eq!(
        kind_of(
            fx.module
                .retry_failed_registration_gate_webhooks(
                    &admin,
                    Some(a.id),
                    Some(oldest.event_id.clone())
                )
                .await
        ),
        KernelError::NotFound,
        "no longer failed"
    );
    let still_b = fx
        .module
        .list_failed_registration_gate_webhook_deliveries(&admin, Some(b.id), None, None)
        .await
        .unwrap();
    assert_eq!(
        still_b.items.len(),
        1,
        "B's delivery with the same event id is untouched"
    );
    assert_eq!(
        kind_of(
            fx.module
                .retry_failed_registration_gate_webhooks(
                    &admin,
                    Some(WebhookEndpointId::new(555)),
                    None
                )
                .await
        ),
        KernelError::NotFound
    );
    assert_eq!(
        kind_of(
            fx.module
                .retry_failed_registration_gate_webhooks(
                    &admin,
                    None,
                    Some(RegistrationGateEventId::new(3001))
                )
                .await
        ),
        KernelError::Validation
    );
    // 一括: 送信先指定と全体。
    assert_eq!(
        fx.module
            .retry_failed_registration_gate_webhooks(&admin, Some(a.id), None)
            .await
            .unwrap(),
        59
    );
    assert_eq!(
        fx.module
            .retry_failed_registration_gate_webhooks(&admin, None, None)
            .await
            .unwrap(),
        1
    );
    assert!(fx
        .audit()
        .await
        .iter()
        .any(|(operation, outcome, detail)| operation == "retry"
            && outcome == "succeeded"
            && detail.as_deref() == Some("requeued=59")));
}

fn env_sender(url: &str) -> HttpWebhookSender {
    HttpWebhookSender::from_values_allowing_insecure_loopback(url, SECRET_A)
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn the_environment_destination_is_listed_read_only_until_a_key_is_configured_and_imported() {
    let server = MockServer::start().await;
    mount_status(&server, 204).await;
    let url = format!("{}/env-hook-abcdefghijkl", server.uri());
    let host = server.address().to_string();

    // 鍵なし: 環境変数の送信先が読み取り専用の送信先として現れる。API から編集・削除できない。
    let fx = Fixture::with(env_sender(&url), AeadWebhookSecretCipher::not_configured()).await;
    let admin = fx.admin();
    let list = fx
        .module
        .list_registration_gate_webhook_endpoints(&admin)
        .await
        .unwrap();
    assert_eq!(list.endpoints.len(), 1);
    let env = &list.endpoints[0];
    assert_eq!(env.id, WebhookEndpointId::ENVIRONMENT);
    assert_eq!(env.source, WebhookEndpointSource::Environment);
    assert!(!env.editable());
    assert_eq!(env.revision, None);
    assert_eq!(env.masked_url, format!("{host}/••••/ijkl"));
    assert_eq!(env.categories, WebhookCategories::all());
    assert!(format!("{env:?}").find("env-hook").is_none());
    assert_eq!(
        rejection_of(
            fx.module
                .update_registration_gate_webhook_endpoint(
                    &admin,
                    WebhookEndpointId::ENVIRONMENT,
                    update_input(1)
                )
                .await
        ),
        WebhookEndpointRejection::EnvironmentEndpointReadOnly
    );
    assert_eq!(
        rejection_of(
            fx.module
                .delete_registration_gate_webhook_endpoint(
                    &admin,
                    WebhookEndpointId::ENVIRONMENT,
                    1
                )
                .await
        ),
        WebhookEndpointRejection::EnvironmentEndpointReadOnly
    );
    // テスト送信は環境変数の送信先でも従来どおり (E15w の運用手順: テスト → 再試行) 動く。
    let outcome = fx
        .module
        .send_registration_gate_webhook_test(&admin, WebhookEndpointId::ENVIRONMENT)
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        WebhookTestOutcome::Sent {
            delivered: true,
            ..
        }
    ));
    fx.insert_failed(0, 901, now()).await;
    assert_eq!(
        fx.module
            .retry_failed_registration_gate_webhooks(
                &admin,
                Some(WebhookEndpointId::ENVIRONMENT),
                None
            )
            .await
            .unwrap(),
        1
    );
    // 鍵がないので取り込まれない。
    assert_eq!(
        fx.module
            .import_environment_webhook_endpoint()
            .await
            .unwrap(),
        None
    );
    assert_eq!(fx.count("registration_gate_webhook_endpoints").await, 0);

    // 鍵あり: 起動時に初期送信先として取り込み、取り込み済みマーカーで冪等。
    let keyed = Module {
        db: PostgresDatabase::new().await.unwrap(),
        sender: env_sender(&url),
        cipher: AeadWebhookSecretCipher::from_key_bytes(KEY_1),
        permission_checker: fx.module.permission_checker.clone(),
    };
    fx.insert_failed(0, 902, now()).await;
    let imported = keyed
        .import_environment_webhook_endpoint()
        .await
        .unwrap()
        .expect("imported");
    assert_eq!(
        keyed.import_environment_webhook_endpoint().await.unwrap(),
        None
    );
    let list = keyed
        .list_registration_gate_webhook_endpoints(&admin)
        .await
        .unwrap();
    assert_eq!(
        list.endpoints.len(),
        1,
        "the read-only entry is replaced by the imported one"
    );
    let view = &list.endpoints[0];
    assert_eq!(view.id, imported);
    assert_eq!(view.source, WebhookEndpointSource::EnvironmentImport);
    assert!(view.editable());
    assert_eq!(view.categories, WebhookCategories::all());
    assert_eq!(view.masked_url, format!("{host}/••••/ijkl"));
    assert_eq!(view.failed_count, 1, "the failed record was carried over");
    assert_eq!(
        kind_of(
            keyed
                .get_registration_gate_webhook_endpoint(&admin, WebhookEndpointId::ENVIRONMENT)
                .await
        ),
        KernelError::NotFound
    );
    assert_eq!(
        kind_of(
            keyed
                .send_registration_gate_webhook_test(&admin, WebhookEndpointId::ENVIRONMENT)
                .await
        ),
        KernelError::NotFound
    );

    // 取り込み後は DB が正本: 管理者が削除しても再取り込みしない (環境変数は無視)。
    keyed
        .delete_registration_gate_webhook_endpoint(&admin, imported, 1)
        .await
        .unwrap();
    assert_eq!(
        keyed.import_environment_webhook_endpoint().await.unwrap(),
        None
    );
    assert!(keyed
        .list_registration_gate_webhook_endpoints(&admin)
        .await
        .unwrap()
        .endpoints
        .is_empty());
    assert!(fx
        .audit()
        .await
        .iter()
        .any(|(operation, outcome, _)| operation == "import" && outcome == "succeeded"));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_disabled_or_misconfigured_environment_destination_is_never_imported() {
    let admin = AuthAccountId::new(ADMIN);
    let cases = [
        HttpWebhookSender::disabled(),
        HttpWebhookSender::from_values(Some("http://insecure.example.net/h"), Some(SECRET_A), None),
        HttpWebhookSender::from_values(Some("https://hooks.example.net/h"), Some("short"), None),
    ];
    for sender in cases {
        let fx = Fixture::with(sender, AeadWebhookSecretCipher::from_key_bytes(KEY_1)).await;
        assert_eq!(
            fx.module
                .import_environment_webhook_endpoint()
                .await
                .unwrap(),
            None
        );
        assert_eq!(fx.count("registration_gate_webhook_endpoints").await, 0);
        assert_eq!(fx.count("registration_gate_webhook_env_import").await, 0);
        let list = fx
            .module
            .list_registration_gate_webhook_endpoints(&admin)
            .await
            .unwrap();
        assert!(list.endpoints.len() <= 1);
    }
    // 設定不備は読み取り専用の送信先として分類だけが見え、状態は failing。
    let fx = Fixture::with(
        HttpWebhookSender::from_values(Some("https://hooks.example.net/h"), Some("short"), None),
        AeadWebhookSecretCipher::not_configured(),
    )
    .await;
    let list = fx
        .module
        .list_registration_gate_webhook_endpoints(&admin)
        .await
        .unwrap();
    let env = &list.endpoints[0];
    assert_eq!(env.health, WebhookEndpointHealth::Failing);
    assert!(env
        .failing_reasons
        .contains(&WebhookFailingReason::ConfigInvalid));
    assert!(env.config_issue.is_some());
    assert_eq!(
        fx.module
            .send_registration_gate_webhook_test(&admin, WebhookEndpointId::ENVIRONMENT)
            .await
            .unwrap(),
        WebhookTestOutcome::Misconfigured
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_unset_environment_destination_is_not_listed_and_tests_as_not_configured() {
    let fx = Fixture::with(
        HttpWebhookSender::disabled(),
        AeadWebhookSecretCipher::not_configured(),
    )
    .await;
    let admin = fx.admin();
    let list = fx
        .module
        .list_registration_gate_webhook_endpoints(&admin)
        .await
        .unwrap();
    assert!(list.endpoints.is_empty());
    assert_eq!(
        fx.module
            .send_registration_gate_webhook_test(&admin, WebhookEndpointId::ENVIRONMENT)
            .await
            .unwrap(),
        WebhookTestOutcome::NotConfigured
    );
}

#[tokio::test]
async fn rejection_reports_carry_a_fixed_code_for_the_http_layer() {
    let report = rejection_report(WebhookEndpointRejection::DuplicateUrl);
    assert_eq!(report.current_context(), &KernelError::Validation);
    assert_eq!(
        report.downcast_ref::<WebhookEndpointRejection>(),
        Some(&WebhookEndpointRejection::DuplicateUrl)
    );
    assert!(report
        .frames()
        .any(|frame| frame.downcast_ref::<String>().map(String::as_str) == Some("duplicate_url")));
}
