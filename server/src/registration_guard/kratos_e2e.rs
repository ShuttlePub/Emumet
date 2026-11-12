//! Real Kratos + production Axum routers + PostgreSQL. Only the external providers are fake.
//! Run via e2e/registration-enforcement/run.sh, never against a shared database.
use super::*;
use crate::auth::{
    encode_test_jwt, generate_test_keys, AuthClaims, JwksCache, OidcConfig, OneOrMany,
};
use crate::handler::AppModule;
use application::service::registration_gate_webhook::DeliverRegistrationGateWebhooksUseCase;
use application::service::registration_protection::RegistrationProtectionReadinessUseCase;
use base64::Engine;
use driver::registration_protection::EnvSource;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use reqwest::{Client, StatusCode as HttpStatus};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

const PUBLIC: &str = "http://127.0.0.1:19433";
const ADMIN: &str = "http://127.0.0.1:19434";
const APP: &str = "http://127.0.0.1:19480";
const SECRET: &str = "fixture-only-guard-secret-at-least-32-bytes";
const PASSWORD: &str = "Fixture-only-Password-4281!";
const GATE: &str = "/api/v1/admin/registration-gate";
const HOOK_DEADLINE: Duration = Duration::from_secs(5);
const DELAYED_OBSERVATION: Duration = Duration::from_secs(10);

#[derive(Default)]
struct HookActivity {
    calls: AtomicUsize,
    inflight: AtomicUsize,
}

struct ActiveHook(Arc<HookActivity>);
impl Drop for ActiveHook {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Smtp {
    port: u16,
    mode: Arc<AtomicUsize>, // 0=healthy, 1=temporary MAIL failure, 2=config loss
    messages: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}
impl Smtp {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mode = Arc::new(AtomicUsize::new(0));
        let messages = Arc::new(Mutex::new(Vec::new()));
        let (m, mail) = (mode.clone(), messages.clone());
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let (mode, messages) = (m.clone(), mail.clone());
                tokio::spawn(async move {
                    let (read, mut write) = socket.into_split();
                    let mut lines = BufReader::new(read).lines();
                    write.write_all(b"220 fixture ESMTP\r\n").await.unwrap();
                    let mut data = None::<String>;
                    while let Ok(Some(line)) = lines.next_line().await {
                        let answer = if let Some(body) = data.as_mut() {
                            if line == "." {
                                messages.lock().unwrap().push(data.take().unwrap());
                                "250 accepted\r\n".to_owned()
                            } else {
                                body.push_str(&line);
                                body.push('\n');
                                continue;
                            }
                        } else if line.starts_with("EHLO") {
                            "250-fixture\r\n250 8BITMIME\r\n".to_owned()
                        } else if line.starts_with("MAIL FROM") {
                            match mode.load(Ordering::SeqCst) {
                                1 => "451 temporary provider outage\r\n",
                                2 => "550 sender configuration invalid\r\n",
                                _ => "250 sender accepted\r\n",
                            }
                            .to_owned()
                        } else if line == "DATA" {
                            data = Some(String::new());
                            "354 send data\r\n".to_owned()
                        } else if line == "QUIT" {
                            let _ = write.write_all(b"221 bye\r\n").await;
                            break;
                        } else {
                            "250 ok\r\n".to_owned()
                        };
                        if write.write_all(answer.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        Self {
            port,
            mode,
            messages,
            task,
        }
    }
    fn code(&self) -> String {
        let messages = self.messages.lock().unwrap();
        let last = messages.last().expect("SMTP DATA must have been received");
        let (headers, body) = last.split_once("\n\n").expect("MIME body");
        let decoded = if headers
            .to_ascii_lowercase()
            .contains("content-transfer-encoding: base64")
        {
            String::from_utf8(
                base64::engine::general_purpose::STANDARD
                    .decode(body.split_whitespace().collect::<String>())
                    .unwrap(),
            )
            .unwrap()
        } else {
            body.replace("=\n", "")
        };
        decoded
            .split(|c: char| !c.is_ascii_digit())
            .find(|s| s.len() == 8) // E15c CODE_DIGITS=8, not Kratos native six-digit codes.
            .expect("eight digit Emumet code in synthetic SMTP mail")
            .to_string()
    }
}
impl Drop for Smtp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Harness {
    module: AppModule,
    source: EnvSource,
    siteverify: String,
    token: String,
    config: Arc<OidcConfig>,
    jwks: Arc<JwksCache>,
    tasks: Vec<JoinHandle<()>>,
    http: Client,
    kratos_db: sqlx::PgPool,
    denied_emails: Mutex<Vec<String>>,
    hook_activity: Arc<HookActivity>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl Harness {
    async fn start(&mut self) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:19480")
            .await
            .unwrap();
        let root = crate::root_router::build_root_router(
            self.module.clone(),
            self.config.clone(),
            self.jwks.clone(),
        );
        self.tasks.push(tokio::spawn(async move {
            axum::serve(
                listener,
                root.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:19481")
            .await
            .unwrap();
        let guard = router(
            GuardConfig {
                listen: listener.local_addr().unwrap(),
                allowed_peers: vec!["127.0.0.1".parse().unwrap()],
                secret: SECRET.to_string(),
                deadline: HOOK_DEADLINE,
                period: Duration::from_secs(30),
            },
            GuardApi(Arc::new(self.module.clone())),
        );
        let activity = self.hook_activity.clone();
        let guard = guard.layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let activity = activity.clone();
                async move {
                    activity.calls.fetch_add(1, Ordering::SeqCst);
                    activity.inflight.fetch_add(1, Ordering::SeqCst);
                    let _active = ActiveHook(activity);
                    next.run(request).await
                }
            },
        ));
        self.tasks.push(tokio::spawn(async move {
            axum::serve(
                listener,
                guard.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        }));
    }
    async fn restart(&mut self, source: EnvSource) {
        for task in self.tasks.drain(..) {
            task.abort();
            let _ = task.await;
        }
        self.module = self
            .module
            .clone()
            .with_registration_fixture(&source, &self.siteverify);
        self.source = source;
        self.start().await;
        self.module.sync_registration_observation().await.unwrap();
        self.module
            .refresh_registration_enforcement()
            .await
            .unwrap();
    }
    async fn get(&self, path: &str) -> Value {
        let response = self
            .http
            .get(format!("{APP}{path}"))
            .bearer_auth(&self.token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), HttpStatus::OK, "{path}");
        response.json().await.unwrap()
    }
    async fn post(&self, path: &str, body: Value, expected: HttpStatus) -> Value {
        let response = self
            .http
            .post(format!("{APP}{path}"))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.bytes().await.unwrap();
        assert_eq!(
            status,
            expected,
            "{path}: {}",
            String::from_utf8_lossy(&bytes)
        );
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }
    async fn command(&self, command: &str, ack: bool, status: HttpStatus) -> Value {
        let state = self.get(GATE).await;
        println!(
            "command {command}: decision={} enforcement={} latch={}",
            state["decision"],
            state["observation"]["enforcement_verified"],
            state["enforcement_lost_latch"]
        );
        self.post(
            &format!("{GATE}/{command}"),
            json!({"revision": state["revision"], "acknowledge_warning": ack}),
            status,
        )
        .await
    }
    async fn probe(&self) {
        // Advance only the test scheduler, not readiness/evidence. The actual probe performs
        // both real public registrations, receipt matching, and admin non-creation checks.
        let mut connection = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        sqlx::query("DELETE FROM registration_enforcement_probe_schedule")
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);
        assert!(self
            .module
            .run_registration_enforcement_probe(time::Duration::seconds(30))
            .await
            .unwrap());
    }
    async fn maintenance(&self) {
        let mut connection = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        sqlx::query("DELETE FROM registration_protection_schedule")
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);
        self.module
            .run_registration_protection_maintenance(time::Duration::seconds(30))
            .await
            .unwrap();
    }
    /// d15-q05: the only bootstrap is the administrator's input-less verification. No widget token and no
    /// canary code is supplied; the provider's real response (Siteverify's rejection of the dummy token
    /// proving the secret valid, SMTP DATA acceptance for the fixed canary) is the evidence.
    async fn bootstrap(&self, smtp: &Smtp, email: bool, bot: bool) {
        if bot {
            let result = self
                .post(
                    "/api/v1/admin/registration-gate/protections/bot/verify",
                    json!({}),
                    HttpStatus::OK,
                )
                .await;
            assert_eq!(result["result"]["outcome"], "business_success");
            assert_eq!(result["readiness"]["status"], "operational");
            assert!(result["pending_challenge"].is_null());
        }
        if email {
            let delivered = smtp.messages.lock().unwrap().len();
            let result = self
                .post(
                    "/api/v1/admin/registration-gate/protections/email/verify",
                    json!({}),
                    HttpStatus::OK,
                )
                .await;
            assert_eq!(result["result"]["outcome"], "business_success");
            assert_eq!(result["readiness"]["status"], "operational");
            assert!(result["pending_challenge"].is_null());
            assert_eq!(
                smtp.messages.lock().unwrap().len(),
                delivered + 1,
                "the canary was really delivered (DATA accepted)"
            );
        }
        self.module.sync_registration_observation().await.unwrap();
    }
    async fn flow(&self, kind: &str) -> (Client, Value) {
        let client = Client::builder()
            .cookie_store(true)
            .no_proxy()
            .pool_max_idle_per_host(0)
            .build()
            .unwrap();
        let response = client
            .get(format!("{PUBLIC}/self-service/registration/{kind}"))
            .header("Accept", "application/json")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), HttpStatus::OK);
        (client, response.json().await.unwrap())
    }
    async fn proof(&self, client: &Client, flow: &Value, email: &str, smtp: &Smtp) -> String {
        let response = client
            .post(format!("{APP}/api/v1/registration/email-challenges"))
            .json(&json!({"email":email,"flow_id":flow["id"]}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), HttpStatus::ACCEPTED);
        let challenge: Value = response.json().await.unwrap();
        let response = client
            .post(format!(
                "{APP}/api/v1/registration/email-challenges/{}/confirm",
                challenge["challenge_id"].as_str().unwrap()
            ))
            .json(&json!({"code":smtp.code()}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), HttpStatus::OK);
        response.json::<Value>().await.unwrap()["proof"]
            .as_str()
            .unwrap()
            .to_string()
    }
    async fn submit(
        &self,
        client: &Client,
        flow: &Value,
        email: &str,
        payload: Value,
        expected: bool,
        form: bool,
    ) {
        if !expected {
            self.denied_emails.lock().unwrap().push(email.to_string());
        }
        let before = self.identity_count(email).await;
        assert_eq!(before, 0, "synthetic address must be fresh");
        let csrf = flow["ui"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["attributes"]["name"] == "csrf_token")
            .map(|n| n["attributes"]["value"].clone());
        let mut body = json!({"method":"password", "password":PASSWORD,"traits":{"email":email},"transient_payload":payload});
        if let Some(csrf) = csrf {
            body["csrf_token"] = csrf;
        }
        let request = client
            .post(flow["ui"]["action"].as_str().unwrap())
            .header("Accept", "application/json");
        let response = if form {
            let mut fields = vec![
                ("method".to_string(), "password".to_string()),
                ("password".into(), PASSWORD.into()),
                ("traits.email".into(), email.into()),
            ];
            if let Some(csrf) = body["csrf_token"].as_str() {
                fields.push(("csrf_token".into(), csrf.into()));
            }
            for (key, value) in payload.as_object().unwrap() {
                fields.push((
                    format!("transient_payload.{key}"),
                    value.as_str().unwrap().to_string(),
                ));
            }
            request.form(&fields).send().await.unwrap()
        } else {
            request.json(&body).send().await.unwrap()
        };
        let status = response.status();
        assert_eq!(
            self.identity_count(email).await,
            i64::from(expected),
            "identity persistence for {email}, HTTP {status}"
        );
        if expected {
            assert!(status.is_success(), "{email}: {status}");
        } else {
            assert!(!status.is_success(), "denied {email}");
        }
        println!("registration {email}: created={expected} HTTP={status}");
    }
    async fn registration(
        &self,
        name: &str,
        kind: &str,
        email_proof: bool,
        bot: bool,
        smtp: &Smtp,
        expected: bool,
    ) {
        let (client, flow) = self.flow(kind).await;
        let email = format!("{name}@example.test");
        let mut payload = json!({});
        if email_proof {
            payload["email_proof"] = json!(self.proof(&client, &flow, &email, smtp).await);
        }
        if bot {
            payload["bot_token"] = json!(format!("valid-{}", uuid::Uuid::new_v4()));
        }
        self.submit(&client, &flow, &email, payload, expected, false)
            .await;
    }
    async fn wait_for_hook_quiescence(&self) {
        tokio::time::timeout(HOOK_DEADLINE * 2, async {
            while self.hook_activity.inflight.load(Ordering::SeqCst) != 0 {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("all observed hook requests must finish before post-processing checks");
    }

    async fn observe_denied_after_processing(&self, phase: &str) {
        self.wait_for_hook_quiescence().await;
        // Two hook deadlines and longer than the fixture's six-second Siteverify stall.
        // A response-time absence check alone does not exclude a later persist.
        assert!(DELAYED_OBSERVATION >= HOOK_DEADLINE * 2);
        let denied = self.denied_emails.lock().unwrap().clone();
        assert!(!denied.is_empty());
        let began = std::time::Instant::now();
        loop {
            assert_eq!(self.hook_activity.inflight.load(Ordering::SeqCst), 0);
            for email in &denied {
                assert_eq!(
                    self.identity_count(email).await,
                    0,
                    "late identity persistence after denied registration: {email} ({phase})"
                );
            }
            if began.elapsed() >= DELAYED_OBSERVATION {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        println!(
            "D15 denied_identity_reobserved_{phase} emails={} window_secs={} hook_inflight=0 ok",
            denied.len(),
            began.elapsed().as_secs()
        );
    }

    async fn identity_count(&self, email: &str) -> i64 {
        let rows: Value = self
            .http
            .get(format!("{ADMIN}/admin/identities?page_size=250"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let admin_count = rows
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["traits"]["email"] == email)
            .count() as i64;
        let db_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM identities WHERE traits::jsonb ->> 'email' = $1",
        )
        .bind(email)
        .fetch_one(&self.kratos_db)
        .await
        .unwrap();
        assert_eq!(
            admin_count, db_count,
            "Kratos admin/DB persistence must agree"
        );
        db_count
    }
}

fn source(smtp: &Smtp, email: bool, bot: bool) -> EnvSource {
    EnvSource::new()
        .with("REGISTRATION_SMTP_HOST", "127.0.0.1")
        .with("REGISTRATION_SMTP_PORT", &smtp.port.to_string())
        .with("REGISTRATION_SMTP_TLS", "plaintext")
        .with("REGISTRATION_EMAIL_FROM", "guard@example.test")
        .with("REGISTRATION_EMAIL_CANARY_ADDRESS", "canary@example.test")
        .with(
            "REGISTRATION_EMAIL_ENABLED",
            if email { "true" } else { "false" },
        )
        .with("REGISTRATION_TURNSTILE_SITE_KEY", "fixture-site")
        .with("REGISTRATION_TURNSTILE_SECRET", "fixture-secret")
        .with("REGISTRATION_TURNSTILE_EXPECTED_HOSTNAME", "guard.example")
        .with("REGISTRATION_TURNSTILE_EXPECTED_ACTION", "register")
        .with(
            "REGISTRATION_TURNSTILE_ENABLED",
            if bot { "true" } else { "false" },
        )
}

async fn kratos_hook(enabled: bool) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let runtime = root.join("e2e/registration-enforcement/runtime");
    let mut config: Value =
        serde_json::from_slice(&std::fs::read(runtime.join("guarded.json")).unwrap()).unwrap();
    if !enabled {
        config["selfservice"]["flows"]["registration"]["after"]["password"]["hooks"] = json!([]);
    }
    std::fs::write(
        runtime.join("kratos.json"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
    let engine = std::env::var("REGISTRATION_E2E_ENGINE").unwrap_or_else(|_| "docker".into());
    let output = tokio::process::Command::new(engine)
        .args([
            "compose",
            "-p",
            "emumet-registration-enforcement-e15d",
            "-f",
        ])
        .arg(root.join("e2e/registration-enforcement/compose.yml"))
        .args(["restart", "kratos"])
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "fixture restart failed");
    let client = Client::new();
    for _ in 0..90 {
        if client
            .get(format!("{PUBLIC}/health/ready"))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("Kratos did not restart");
}

#[ignore = "dedicated real Kratos fixture; run e2e/registration-enforcement/run.sh"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_kratos_d15_q01_through_q11() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .with_test_writer()
        .try_init();
    assert_eq!(std::env::var("REGISTRATION_KRATOS_E2E").as_deref(), Ok("1"));
    kernel::ensure_generator_initialized();
    // Fixed loopback URLs belong exclusively to the disposable compose project.
    let fixture_client = Client::new();
    let rows: Vec<Value> = fixture_client
        .get(format!("{ADMIN}/admin/identities?page_size=250"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for identity in rows {
        let email = identity["traits"]["email"].as_str().unwrap();
        assert!(
            email.ends_with("@example.test") || email.ends_with("@registration-canary.invalid")
        );
        assert!(fixture_client
            .delete(format!(
                "{ADMIN}/admin/identities/{}",
                identity["id"].as_str().unwrap()
            ))
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
    }
    let smtp = Smtp::start().await;
    let provider = MockServer::start().await;
    let mode = Arc::new(AtomicUsize::new(0));
    let response_mode = mode.clone();
    Mock::given(method("POST")).and(path("/siteverify")).respond_with(move |request: &wiremock::Request| {
        match response_mode.load(Ordering::SeqCst) {
            1 => ResponseTemplate::new(503),
            2 => ResponseTemplate::new(200).set_body_json(json!({"success":false,"error-codes":["invalid-input-secret"]})),
            _ => {
                let form: std::collections::HashMap<_,_> = url::form_urlencoded::parse(&request.body).into_owned().collect();
                if form.get("response").is_some_and(|s|s.starts_with("valid-")) {
                    ResponseTemplate::new(200).set_body_json(json!({"success":true,"hostname":"guard.example","action":"register","challenge_ts":time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap()}))
                } else if form.get("response").is_some_and(|s|s == "timeout") { ResponseTemplate::new(503).set_delay(Duration::from_secs(6)) }
                else { ResponseTemplate::new(200).set_body_json(json!({"success":false,"error-codes":["invalid-input-response"]})) }
            }
        }
    }).mount(&provider).await;
    Mock::given(method("POST"))
        .and(path("/relation-tuples/check"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"allowed":true})))
        .mount(&provider)
        .await;
    Mock::given(method("PUT"))
        .and(path("/admin/relation-tuples"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&provider)
        .await;
    Mock::given(method("POST"))
        .and(path("/webhook"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&provider)
        .await;
    let siteverify = format!("{}/siteverify", provider.uri());
    let settings = source(&smtp, false, true);
    let key_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(key_file.path(), b"fixture-only-master-key-password").unwrap();
    let module = AppModule::new_for_test_urls(
        provider.uri(),
        PUBLIC.to_string(),
        provider.uri(),
        provider.uri(),
    )
    .await
    .unwrap()
    .with_registration_fixture(&settings, &siteverify)
    .with_password_provider(driver::crypto::FilePasswordProvider::with_paths(
        key_file.path(),
        key_file.path(),
    ))
    .with_webhook_sender(
        driver::webhook::HttpWebhookSender::from_values_allowing_insecure_loopback(
            &format!("{}/webhook", provider.uri()),
            "fixture-webhook-secret-at-least-32-bytes",
        ),
    );
    let mut db = module.database_connection().connection().await.unwrap();
    sqlx::query("TRUNCATE registration_gate_webhook_attempts, registration_gate_webhook_deliveries, registration_gate_webhook_endpoints, registration_gate_webhook_endpoint_audit, registration_gate_webhook_env_import, registration_gate_webhook_state, registration_gate_event_reads, registration_gate_events, registration_gate_audit, registration_gate_state, registration_enforcement_probe_state, registration_enforcement_probe_schedule, registration_enforcement_canaries, registration_protection_readiness, registration_email_challenges, registration_email_send_log, registration_protection_schedule CASCADE").execute(&mut *db).await.unwrap();
    drop(db);
    let keys = generate_test_keys();
    let issuer = "https://issuer.e15d.example";
    let claims = AuthClaims {
        iss: issuer.into(),
        sub: uuid::Uuid::new_v4().to_string(),
        aud: OneOrMany::One("account".into()),
        exp: (time::OffsetDateTime::now_utc().unix_timestamp() + 3600) as u64,
    };
    let token = encode_test_jwt(&claims, &keys.encoding_key, &keys.kid);
    let config = Arc::new(OidcConfig {
        issuer_url: issuer.into(),
        expected_audience: "account".into(),
        jwks_refetch_interval_secs: 0,
    });
    let jwks = Arc::new(JwksCache::new_with_jwks(issuer.into(), keys.jwk_set));
    let kratos_db = sqlx::PgPool::connect(
        &std::env::var("DATABASE_URL")
            .unwrap()
            .replace("/emumet", "/kratos"),
    )
    .await
    .unwrap();
    let mut app = Harness {
        kratos_db,
        denied_emails: Mutex::new(Vec::new()),
        hook_activity: Arc::new(HookActivity::default()),
        module,
        source: settings,
        siteverify,
        token,
        config,
        jwks,
        tasks: vec![],
        http: Client::builder()
            .no_proxy()
            .pool_max_idle_per_host(0)
            .build()
            .unwrap(),
    };
    app.start().await;
    // E15w enables a baseline rather than retroactively delivering old events.
    // Start it before the scenarios, as production startup does.
    crate::webhook_worker::prepare_baseline(&app.module)
        .await
        .unwrap();
    app.probe().await;
    app.module.sync_registration_observation().await.unwrap();
    app.command("open", false, HttpStatus::UNPROCESSABLE_ENTITY)
        .await;
    app.registration("q05-unverified", "api", false, true, &smtp, false)
        .await;
    // Configuration alone (and the periodic worker, which only renews already-verified generations) never
    // makes a protection ready or lets the gate open.
    app.maintenance().await;
    app.command("open", false, HttpStatus::UNPROCESSABLE_ENTITY)
        .await;
    app.registration(
        "q05-unverified-after-maintenance",
        "api",
        false,
        true,
        &smtp,
        false,
    )
    .await;
    // The retired widget-token / canary-code inputs are rejected, not silently ignored.
    for body in [
        json!({"token":"valid-bootstrap"}),
        json!({"challenge_id":"x","code":"12345678"}),
    ] {
        app.post(
            "/api/v1/admin/registration-gate/protections/bot/verify",
            body,
            HttpStatus::BAD_REQUEST,
        )
        .await;
    }
    // The administrator's input-less verification (the provider really answered) is what opens it.
    app.bootstrap(&smtp, false, true).await;
    println!("D15 q05_api_verified_readiness ok");
    app.command("open", false, HttpStatus::OK).await;
    app.registration("q01-bot-only", "api", false, true, &smtp, true)
        .await;
    app.registration("q01-bot-missing", "browser", false, false, &smtp, false)
        .await;
    let (browser, flow) = app.flow("browser").await;
    app.submit(
        &browser,
        &flow,
        "q01-browser-form-no-proof@example.test",
        json!({}),
        false,
        true,
    )
    .await;
    app.restart(source(&smtp, true, true)).await;
    app.bootstrap(&smtp, true, false).await;
    app.registration("q01-both", "browser", true, true, &smtp, true)
        .await;
    app.registration("q01-both-missing-email", "api", false, true, &smtp, false)
        .await;
    app.restart(source(&smtp, true, false)).await;
    app.registration("q01-email-only", "api", true, false, &smtp, true)
        .await;
    println!("D15 q01_either_email_or_bot_and_both_effective ok");
    app.restart(source(&smtp, false, false)).await;
    app.registration("q02-config-loss", "api", false, false, &smtp, false)
        .await;
    app.command("open", false, HttpStatus::UNPROCESSABLE_ENTITY)
        .await;
    println!("D15 q02_config_loss_stops ok");
    app.restart(source(&smtp, true, true)).await;
    app.maintenance().await;
    app.registration("q03-restored-latched", "browser", false, true, &smtp, false)
        .await;
    app.command("open", false, HttpStatus::OK).await;
    app.registration("q03-manual-reopen", "api", true, true, &smtp, true)
        .await;
    let old = app.flow("api").await;
    app.command("close", false, HttpStatus::OK).await;
    app.submit(
        &old.0,
        &old.1,
        "old-flow-closed@example.test",
        json!({"bot_token":"valid-old","override":true}),
        false,
        false,
    )
    .await;
    app.maintenance().await;
    assert_eq!(app.get(GATE).await["decision"], "admin_closed");
    app.command("open", false, HttpStatus::OK).await;
    println!("D15 q03_manual_reopen_and_old_flow_closure ok");
    mode.store(1, Ordering::SeqCst);
    app.maintenance().await;
    app.registration("q07-email-working", "api", true, false, &smtp, true)
        .await;
    app.registration("q07-email-still-required", "api", false, true, &smtp, false)
        .await;
    mode.store(0, Ordering::SeqCst);
    smtp.mode.store(1, Ordering::SeqCst);
    app.maintenance().await;
    app.registration("q07-bot-working", "browser", false, true, &smtp, true)
        .await;
    println!("D15 q07_single_provider_outage_continues ok");
    mode.store(1, Ordering::SeqCst);
    app.maintenance().await;
    assert_eq!(app.get(GATE).await["decision"], "temporary_reject");
    app.registration("q06-both-down", "api", false, true, &smtp, false)
        .await;
    mode.store(0, Ordering::SeqCst);
    smtp.mode.store(0, Ordering::SeqCst);
    app.maintenance().await;
    app.registration("q06-auto-resume", "browser", true, true, &smtp, true)
        .await;
    println!("D15 q06_temporary_outage_auto_resume ok");
    app.restart(source(&smtp, false, false)).await;
    app.command("override", false, HttpStatus::BAD_REQUEST)
        .await;
    app.command("override", true, HttpStatus::OK).await;
    app.registration("q09-override-unprotected", "api", false, false, &smtp, true)
        .await;
    println!("D15 q09_warning_ack_required ok");
    let override_before = app.get(GATE).await["override"].clone();
    app.restart(source(&smtp, false, true)).await;
    app.bootstrap(&smtp, false, true).await;
    assert_eq!(app.get(GATE).await["override"], override_before);
    app.registration(
        "q10-override-effective-missing",
        "browser",
        false,
        false,
        &smtp,
        false,
    )
    .await;
    app.registration("q10-override-effective", "api", false, true, &smtp, true)
        .await;
    app.command("override/revoke", false, HttpStatus::OK).await;
    assert_eq!(app.get(GATE).await["override"]["active"], false);
    app.command("close", false, HttpStatus::OK).await;
    let (invitation_client, invitation_flow) = app.flow("api").await;
    app.submit(
        &invitation_client,
        &invitation_flow,
        "q04-fake-invitation@example.test",
        json!({"bot_token":"valid-invitation","invitation":"self-asserted","override":true}),
        false,
        false,
    )
    .await;
    println!("D15 q04_no_public_invitation_bypass_no_invitation_feature ok");
    app.probe().await; // Refresh evidence before the isolated non-registration regressions.
    for phase in ["closed", "provider_outage"] {
        let expected_decision = if phase == "provider_outage" {
            app.command("open", false, HttpStatus::OK).await;
            mode.store(1, Ordering::SeqCst);
            smtp.mode.store(1, Ordering::SeqCst);
            app.maintenance().await;
            app.registration("q08-outage-denied", "api", false, true, &smtp, false)
                .await;
            "temporary_reject"
        } else {
            "admin_closed"
        };
        assert_eq!(app.get(GATE).await["decision"], expected_decision);
        app.wait_for_hook_quiescence().await;
        let before = app.hook_activity.calls.load(Ordering::SeqCst);
        // The login subject was created by the real protected registration above.
        let flow: Value = app
            .http
            .get(format!("{PUBLIC}/self-service/login/api"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let login = app.http.post(flow["ui"]["action"].as_str().unwrap()).json(&json!({"method":"password","identifier":"q01-bot-only@example.test","password":PASSWORD})).send().await.unwrap();
        assert_eq!(login.status(), HttpStatus::OK);
        let logged_in: Value = login.json().await.unwrap();
        let account_claims = AuthClaims {
            iss: claims.iss.clone(),
            sub: logged_in["session"]["identity"]["id"]
                .as_str()
                .expect("real login identity")
                .to_string(),
            aud: OneOrMany::One("account".into()),
            exp: claims.exp,
        };
        app.token = encode_test_jwt(&account_claims, &keys.encoding_key, &keys.kid);
        for name in ["First", "Second"] {
            app.post(
                "/api/v1/accounts",
                json!({
                    "name": format!("e15d-{name}-{}", uuid::Uuid::new_v4()),
                    "is_bot": false,
                    "profile_username": format!("u{}", uuid::Uuid::new_v4().simple()),
                }),
                HttpStatus::CREATED,
            )
            .await;
        }

        app.wait_for_hook_quiescence().await;
        assert_eq!(
            app.hook_activity.calls.load(Ordering::SeqCst),
            before,
            "login and authenticated Account additions must not call registration hook ({phase})"
        );
        println!("D15 q08_{phase}_login_accounts_hook_delta_zero ok");
        if phase == "provider_outage" {
            mode.store(0, Ordering::SeqCst);
            smtp.mode.store(0, Ordering::SeqCst);
            app.maintenance().await;
            app.command("close", false, HttpStatus::OK).await;
        }
    }
    println!("D15 q08_login_and_authenticated_account_additions ok");
    println!("D15 q10_override_persists_until_revoked_and_enforces_ready_protection ok");
    app.command("open", false, HttpStatus::OK).await;
    let (client, flow) = app.flow("api").await;
    let began = std::time::Instant::now();
    app.submit(
        &client,
        &flow,
        "provider-timeout@example.test",
        json!({"bot_token":"timeout"}),
        false,
        false,
    )
    .await;
    assert!(began.elapsed() < Duration::from_secs(10));
    app.observe_denied_after_processing("timeout_drained").await;
    app.maintenance().await;
    // Hook deletion really creates reserved identities. Monitor persists the stop/events
    // before cleanup; a restored hook/healthy providers never clears the stop latch.
    app.command("override", true, HttpStatus::OK).await;
    kratos_hook(false).await;
    app.probe().await;
    let stopped = app.get(GATE).await;
    assert_eq!(stopped["decision"], "stopped");
    assert_eq!(stopped["override"]["active"], true);
    assert_eq!(stopped["enforcement_lost_latch"], true);
    let rows: Value = app
        .http
        .get(format!("{ADMIN}/admin/identities?page_size=250"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .all(|r| !r["traits"]["email"]
            .as_str()
            .unwrap()
            .ends_with("@registration-canary.invalid")));
    app.restart(source(&smtp, false, false)).await;
    assert_eq!(app.get(GATE).await["decision"], "stopped");
    kratos_hook(true).await;
    app.probe().await;
    app.registration(
        "enforcement-restored-latched",
        "api",
        false,
        false,
        &smtp,
        false,
    )
    .await;
    app.command("open", false, HttpStatus::UNPROCESSABLE_ENTITY)
        .await;
    app.command("override", false, HttpStatus::BAD_REQUEST)
        .await;
    app.command("override", true, HttpStatus::OK).await;
    app.registration(
        "enforcement-manual-override-resume",
        "api",
        false,
        false,
        &smtp,
        true,
    )
    .await;
    println!("D15 enforcement_hook_removed_negative_canary_restart_manual_reack ok");
    let notifications = app.get(&format!("{GATE}/notifications?limit=100")).await;
    assert!(notifications["unread_count"].as_u64().unwrap() > 0);
    let text = notifications.to_string();
    for kind in [
        "protection_degraded",
        "outage_started",
        "outage_recovered",
        "protection_lost_stopped",
        "enforcement_lost_stopped",
        "enforcement_restored",
        "override_started",
        "override_revoked",
    ] {
        assert!(text.contains(kind), "missing notification {kind}");
    }
    app.module
        .deliver_registration_gate_webhooks(
            "e15d-test",
            &kernel::prelude::entity::WebhookDeliveryPolicy {
                batch_size: 100,
                ..Default::default()
            },
            &|| 0.5,
            &|| false,
        )
        .await
        .unwrap();
    let requests = provider.received_requests().await.unwrap();
    let hooks: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path() == "/webhook")
        .collect();
    assert!(!hooks.is_empty());
    for kind in [
        "enforcement_lost_stopped",
        "enforcement_restored",
        "protection_degraded",
        "outage_started",
        "outage_recovered",
    ] {
        assert!(
            hooks
                .iter()
                .any(|r| String::from_utf8_lossy(&r.body).contains(kind)),
            "missing external webhook {kind}"
        );
    }
    println!("D15 q11_persistent_notifications_unread_and_e15w_webhook ok");
    app.observe_denied_after_processing("suite_finished").await;
}
