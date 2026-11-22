//! B1 process tests intentionally seed links directly; real user registration is T1's scope.
#![cfg(all(unix, feature = "test-mode"))]
use driver::database::{isolated_db::IsolatedDatabase, PostgresDatabase};
use kernel::{
    interfaces::{
        database::*, event::EventApplier, home_link::*, post_relay::*, read_model::*, repository::*,
    },
    prelude::entity::*,
};
use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    process::Stdio,
    time::{Duration, Instant},
};
use time::OffsetDateTime;
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};
const PUBLIC: &str = "https://emumet.example";
fn database_url(db: &IsolatedDatabase) -> String {
    let mut u = url::Url::parse(&std::env::var("DATABASE_URL").expect("B1 DB tests must not skip"))
        .unwrap();
    u.set_path(db.pool.connect_options().get_database().unwrap());
    u.to_string()
}
fn working_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let password = dir.path().join("master-key-password");
    std::fs::write(&password, b"b1-process-test-only-password").unwrap();
    std::fs::set_permissions(password, std::fs::Permissions::from_mode(0o600)).unwrap();
    dir
}
struct Running {
    child: Child,
    base: String,
    log: std::path::PathBuf,
}
impl Running {
    async fn start(
        db: &IsolatedDatabase,
        dir: &tempfile::TempDir,
        ory: &str,
        home: &str,
        overrides: &[(&str, &str)],
    ) -> Self {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let address = format!("127.0.0.1:{port}");
        let log = dir.path().join(format!("runtime-{port}.log"));
        let output = std::fs::File::create(&log).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_server"));
        command
            .current_dir(dir.path())
            .kill_on_drop(true)
            .env("DATABASE_URL", database_url(db))
            .env("PUBLIC_BASE_URL", PUBLIC)
            .env("HTTP_LISTEN_ADDR", &address)
            .env("HYDRA_ISSUER_URL", ory)
            .env("HYDRA_ADMIN_URL", ory)
            .env("KRATOS_PUBLIC_URL", ory)
            .env("KETO_READ_URL", ory)
            .env("KETO_WRITE_URL", ory)
            .env("AWS_EC2_METADATA_DISABLED", "true")
            .env("EMUMET_FILE_LOG_ENABLED", "false")
            .env("EMUMET_TEST_MODE_TOKEN", "b1-process-test-token")
            .env("AP_TEST_ALLOWED_FETCH_HOSTS", "127.0.0.1")
            .env(
                "EMUMET_TEST_RELAY_ORIGINS",
                serde_json::to_string(&[home]).unwrap(),
            )
            .env("POST_RELAY_ALLOWED_ORIGINS", "[]")
            .env("POST_RELAY_SENDER_ENABLED", "true")
            .env("POST_RELAY_INBOX_ENABLED", "true")
            .env("JWKS_TIMEOUT_MS", "60000")
            .env("SERVER_SHUTDOWN_TIMEOUT_SECS", "3")
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(output.try_clone().unwrap())
            .stderr(output);
        for (key, value) in overrides {
            command.env(key, value);
        }
        let child = command.spawn().unwrap();
        let mut server = Self {
            child,
            base: format!("http://{address}"),
            log,
        };
        let client = client();
        // Includes one-time RSA key generation. Do not conflate CPU-bound provisioning with
        // a JWKS dependency: the stalled-Ory cases separately assert the stricter 30s bound.
        let result = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                assert!(
                    server.child.try_wait().unwrap().is_none(),
                    "server failed: {}",
                    server.logs()
                );
                if client
                    .get(format!("{}/healthz", server.base))
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await;
        assert!(
            result.is_ok(),
            "startup including one-time key provisioning exceeded its bound: {}",
            server.logs()
        );
        server
    }
    fn logs(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap()
    }
    fn signal(&self, signal: &str) {
        assert!(std::process::Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(self.child.id().unwrap().to_string())
            .status()
            .unwrap()
            .success());
    }
    async fn term(&mut self) {
        let started = Instant::now();
        self.signal("TERM");
        let status = tokio::time::timeout(Duration::from_secs(6), self.child.wait())
            .await
            .expect("SIGTERM overall deadline")
            .unwrap();
        assert!(status.success(), "{}", self.logs());
        assert!(started.elapsed() < Duration::from_secs(6));
    }
    async fn kill(&mut self) {
        self.child.kill().await.unwrap();
        self.child.wait().await.unwrap();
    }
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap()
}
async fn physical(db: &IsolatedDatabase) {
    let mut tx = db.database.get_transaction().await.unwrap();
    db.database
        .post_relay_store()
        .report_capacity(
            tx.connection(),
            &CapacityReport {
                free_bytes: 50 * 1024 * 1024 * 1024,
                total_bytes: 100 * 1024 * 1024 * 1024,
                sampled_at: OffsetDateTime::now_utc(),
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
}
async fn enqueue(db: &IsolatedDatabase, profile: &Profile, receipt: &str) {
    physical(db).await;
    let store = db.database.post_relay_store();
    let mut tx = db.database.get_transaction().await.unwrap();
    let recipient = store
        .prepare_admission(tx.connection(), profile.id())
        .await
        .unwrap();
    assert_eq!(
        store
            .enqueue(
                tx.connection(),
                &RelayAdmission {
                    profile_id: profile.id().clone(),
                    receipt: receipt.into(),
                    received_at: OffsetDateTime::now_utc(),
                    recipient,
                    envelope: br#"{"b1":"immutable process test"}"#.to_vec()
                }
            )
            .await
            .unwrap(),
        Admission::Queued
    );
    tx.commit().await.unwrap();
}
async fn wait_no_jobs(db: &IsolatedDatabase) {
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM post_relay_jobs")
                .fetch_one(&db.pool)
                .await
                .unwrap();
            if count == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("delivery/reclaimed lease must complete");
}
async fn wait_claim(db: &IsolatedDatabase, profile: &Profile, authorized: bool) -> RelayLease {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let row:Option<(String,i64,OffsetDateTime,bool)> = sqlx::query_as("SELECT receipt,token,lease_until,authorized_at IS NOT NULL FROM post_relay_jobs WHERE lease_until IS NOT NULL").fetch_optional(&db.pool).await.unwrap();
            if let Some((receipt,token,lease_until,auth)) = row {if auth==authorized { return RelayLease {receipt,token,lease_until,profile_id:profile.id().clone(),queue_wait_ns:0};}}
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("claim/authorization boundary")
}
async fn signed_inbox(
    server: &Running,
    profile: &Profile,
    peer: &MockServer,
) -> (reqwest::Response, Vec<u8>) {
    use driver::{crypto::Rsa2048RawGenerator, http_signing::HttpSignerImpl};
    use kernel::interfaces::{
        crypto::{RawKeyGenerator, SigningAlgorithm},
        http_signing::{HttpSigner, HttpSigningRequest},
    };
    use sha2::Digest;
    let keys = Rsa2048RawGenerator.generate_raw().unwrap();
    let actor = format!("{}/actor", peer.uri());
    let key = format!("{actor}#key");
    Mock::given(path("/actor")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "id":actor,"type":"Person","inbox":format!("{}/inbox",peer.uri()),"publicKey":{"id":key,"owner":actor,"publicKeyPem":keys.public_key_pem}
    }))).mount(peer).await;
    let target = format!("/ap/profiles/{}/inbox", profile.nanoid().as_ref());
    let body=serde_json::to_vec(&serde_json::json!({"type":"Create","id":format!("{}/create/{}",peer.uri(),uuid::Uuid::new_v4()),"actor":actor,"object":{"type":"Note","content":"B1 outage"}})).unwrap();
    let digest = format!(
        "SHA-256={}",
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            sha2::Sha256::digest(&body)
        )
    );
    let headers = HashMap::from([
        ("host".into(), "emumet.example".into()),
        (
            "date".into(),
            httpdate::fmt_http_date(std::time::SystemTime::now()),
        ),
        ("digest".into(), digest),
    ]);
    let signed = HttpSignerImpl
        .sign(
            &HttpSigningRequest {
                method: "POST".into(),
                url: format!("{PUBLIC}{target}"),
                headers: headers.clone(),
                body: Some(body.clone()),
            },
            &keys.private_key_pem,
            &key,
            &SigningAlgorithm::Rsa2048,
        )
        .await
        .unwrap();
    let mut request = client()
        .post(format!("{}{target}", server.base))
        .header("content-type", "application/activity+json")
        .body(body.clone());
    for (key, value) in headers.into_iter().chain(signed.cavage_headers) {
        request = request.header(key, value);
    }
    (request.send().await.unwrap(), body)
}

async fn fixture(db: &PostgresDatabase, origin: &str) -> (Account, Vec<Profile>) {
    kernel::ensure_generator_initialized();
    let mut c = db.connection().await.unwrap();
    let id = AccountId::default();
    let command = Account::create(
        id.clone(),
        AccountName::new(format!("relay{}", id.as_ref())),
        AccountIsBot::new(false),
        Nanoid::default(),
        AuthAccountId::default(),
    );
    let e = db.account_repository().save(&mut c, command).await.unwrap();
    let mut a = None;
    Account::apply(&mut a, e).unwrap();
    let a = a.unwrap();
    db.account_read_model().create(&mut c, &a).await.unwrap();
    let e = db
        .profile_repository()
        .save(
            &mut c,
            Profile::create(
                ProfileId::new(kernel::generate_id()),
                id.clone(),
                None,
                None,
                None,
                None,
                Nanoid::default(),
            ),
        )
        .await
        .unwrap();
    let mut p = None;
    Profile::apply(&mut p, e).unwrap();
    let p = p.unwrap();
    let mut t = db.get_transaction().await.unwrap();
    db.home_link_store()
        .lock_accounts(t.connection(), std::slice::from_ref(&id))
        .await
        .unwrap();
    db.profile_read_model()
        .create(t.connection(), &p)
        .await
        .unwrap();
    sqlx::query("UPDATE profiles SET username=$2 WHERE id=$1")
        .bind(p.id().as_ref())
        .bind(format!("relay_{}", p.id().as_ref()))
        .execute(&mut **t.connection())
        .await
        .unwrap();
    db.home_link_store()
        .lock_profiles(t.connection(), std::slice::from_ref(p.id()))
        .await
        .unwrap();
    {
        let o = db
            .home_link_store()
            .owner(t.connection(), p.id(), &id)
            .await
            .unwrap();
        db.home_link_store()
            .register(
                t.connection(),
                &o,
                "https://emumet.example/ap/profiles/relay",
                origin,
            )
            .await
            .unwrap();
    }
    t.commit().await.unwrap();
    (a, vec![p])
}

#[tokio::test]
async fn post_relay_runtime_cold_start_outages_serve_instance_and_forward_signed_inbox() {
    let db = IsolatedDatabase::create().await;
    let home = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/internal/v1/emumet/activities"))
        .respond_with(ResponseTemplate::new(202))
        .mount(&home)
        .await;
    let (_, profiles) = fixture(&db.database, &home.uri()).await;
    let profile = &profiles[0];
    let dir = working_dir();
    let refused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let refused_url = format!("http://{}", refused.local_addr().unwrap());
    drop(refused);
    let discovery_stall = MockServer::start().await;
    Mock::given(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(120)))
        .mount(&discovery_stall)
        .await;
    let fetch_stall = MockServer::start().await;
    Mock::given(path("/.well-known/openid-configuration"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"jwks_uri":format!("{}/keys",fetch_stall.uri())}),
            ),
        )
        .mount(&fetch_stall)
        .await;
    Mock::given(path("/keys"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(120)))
        .mount(&fetch_stall)
        .await;
    for (index, ory) in [refused_url, discovery_stall.uri(), fetch_stall.uri()]
        .into_iter()
        .enumerate()
    {
        let started = Instant::now();
        let mut server = Running::start(&db, &dir, &ory, &home.uri(), &[]).await;
        // First/refused start provisions both key sets; subsequent processes still have a cold
        // JWKS cache, but no random/CPU-dependent RSA generation in this latency assertion.
        if index > 0 {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "cold JWKS startup must not wait for the 60s acquisition deadline: {}",
                server.logs()
            );
        }
        let instance: serde_json::Value = client()
            .get(format!("{}/ap/instance", server.base))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(instance["id"], format!("{PUBLIC}/ap/instance"));
        assert!(instance["publicKey"].is_array());
        for bearer in [None, Some("unverifiable.token.value")] {
            let mut request = client().get(format!("{}/api/v1/me", server.base));
            if let Some(token) = bearer {
                request = request.bearer_auth(token);
            }
            assert_eq!(request.send().await.unwrap().status(), 401);
        }
        physical(&db).await;
        let remote = MockServer::start().await;
        let before = home.received_requests().await.unwrap().len();
        let (response, body) = signed_inbox(&server, profile, &remote).await;
        assert_eq!(response.status(), 202, "{}", response.text().await.unwrap());
        wait_no_jobs(&db).await;
        let requests = home.received_requests().await.unwrap();
        assert_eq!(requests.len(), before + 1, "{}", server.logs());
        let envelope: serde_json::Value =
            serde_json::from_slice(&requests.last().unwrap().body).unwrap();
        assert_eq!(
            base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                envelope["activity"]["raw_body"].as_str().unwrap()
            )
            .unwrap(),
            body
        );
        assert!(requests.last().unwrap().headers.get("signature").is_some());
        server.term().await;
    }
}

#[tokio::test]
async fn post_relay_runtime_empty_migration_and_stopped_sender_preserve_populated_restart() {
    let db = IsolatedDatabase::create_before_migration(0).await;
    let home = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(202))
        .mount(&home)
        .await;
    let dir = working_dir();
    let mut server = Running::start(
        &db,
        &dir,
        "http://127.0.0.1:1",
        &home.uri(),
        &[
            ("POST_RELAY_SENDER_ENABLED", "false"),
            ("POST_RELAY_INBOX_ENABLED", "false"),
        ],
    )
    .await;
    let original = client()
        .get(format!("{}/ap/instance", server.base))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    // The fixture writes both events and their read models. Do this with the child stopped,
    // otherwise its live projector can race the fixture's read-model create (seen in coverage).
    server.term().await;
    let (_, profiles) = fixture(&db.database, &home.uri()).await;
    enqueue(&db, &profiles[0], "stopped-job").await;
    let mut server = Running::start(
        &db,
        &dir,
        "http://127.0.0.1:1",
        &home.uri(),
        &[
            ("POST_RELAY_SENDER_ENABLED", "false"),
            ("POST_RELAY_INBOX_ENABLED", "false"),
        ],
    )
    .await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let token: i64 =
        sqlx::query_scalar("SELECT token FROM post_relay_jobs WHERE receipt='stopped-job'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(token, 0);
    assert!(home.received_requests().await.unwrap().is_empty());
    server.term().await;
    let mut server = Running::start(&db, &dir, "http://127.0.0.1:1", &home.uri(), &[]).await;
    assert_eq!(
        client()
            .get(format!("{}/ap/instance", server.base))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap(),
        original
    );
    wait_no_jobs(&db).await;
    server.term().await;
}

#[tokio::test]
async fn post_relay_runtime_sigterm_stops_claims_before_http_drain_and_bounds_stalled_work() {
    let db = IsolatedDatabase::create().await;
    let home = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(202).set_delay(Duration::from_millis(500)))
        .mount(&home)
        .await;
    let (_, profiles) = fixture(&db.database, &home.uri()).await;
    let profile = &profiles[0];
    let dir = working_dir();
    enqueue(&db, profile, "graceful-in-flight").await;
    let mut graceful = Running::start(
        &db,
        &dir,
        "http://127.0.0.1:1",
        &home.uri(),
        &[("SERVER_SHUTDOWN_TIMEOUT_SECS", "5")],
    )
    .await;
    wait_claim(&db, profile, true).await;
    graceful.term().await;
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM post_relay_jobs")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        remaining, 0,
        "an in-flight success must be persisted during SIGTERM drain"
    );
    assert!(!graceful.logs().contains("Runtime drain deadline exceeded"));
    assert_eq!(home.received_requests().await.unwrap().len(), 1);

    home.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(202).set_delay(Duration::from_secs(120)))
        .mount(&home)
        .await;
    enqueue(&db, profile, "in-flight").await;
    let mut server = Running::start(
        &db,
        &dir,
        "http://127.0.0.1:1",
        &home.uri(),
        &[("SERVER_SHUTDOWN_TIMEOUT_SECS", "2")],
    )
    .await;
    let _lease = wait_claim(&db, profile, true).await;
    // An incomplete body keeps a real axum request alive; outbound HTTP is also stalled.
    let mut socket = tokio::net::TcpStream::connect(server.base.trim_start_matches("http://"))
        .await
        .unwrap();
    socket.write_all(format!("POST /ap/profiles/{}/inbox HTTP/1.1\r\nHost: emumet.example\r\nContent-Type: application/activity+json\r\nContent-Length: 1000\r\n\r\n{{",profile.nanoid().as_ref()).as_bytes()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    server.signal("TERM");
    tokio::time::timeout(Duration::from_secs(1), async {
        while !server
            .logs()
            .contains("Stopping claims and draining runtime")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    enqueue(&db, profile, "after-signal").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let token: i64 =
        sqlx::query_scalar("SELECT token FROM post_relay_jobs WHERE receipt='after-signal'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(token, 0, "no new claims during HTTP drain");
    let status = tokio::time::timeout(Duration::from_secs(4), server.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success(), "{}", server.logs());
    assert!(started.elapsed() < Duration::from_secs(4));
    assert!(
        server.logs().contains("Runtime drain deadline exceeded"),
        "{}",
        server.logs()
    );
    drop(socket);
}

#[tokio::test]
async fn post_relay_runtime_sigkill_before_and_after_authorization_recovers_original_lease() {
    for authorized in [false, true] {
        let db = IsolatedDatabase::create().await;
        let home = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(202).set_delay(Duration::from_secs(120)))
            .mount(&home)
            .await;
        let (account, profiles) = fixture(&db.database, &home.uri()).await;
        let profile = &profiles[0];
        let dir = working_dir();
        let mut bootstrap = Running::start(
            &db,
            &dir,
            "http://127.0.0.1:1",
            &home.uri(),
            &[("POST_RELAY_SENDER_ENABLED", "false")],
        )
        .await;
        bootstrap.term().await;
        enqueue(&db, profile, "crashed-job").await;
        // Prevent authorization after the independently committed claim (P0).
        let mut blocker = db.database.get_transaction().await.unwrap();
        if !authorized {
            db.database
                .home_link_store()
                .lock_accounts(blocker.connection(), std::slice::from_ref(account.id()))
                .await
                .unwrap();
        }
        let mut server = Running::start(&db, &dir, "http://127.0.0.1:1", &home.uri(), &[]).await;
        let old = wait_claim(&db, profile, authorized).await;
        let expiry: OffsetDateTime = sqlx::query_scalar(
            "SELECT automatic_expires_at FROM post_relay_bodies WHERE receipt='crashed-job'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        server.kill().await;
        drop(blocker);
        home.reset().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(202))
            .mount(&home)
            .await;
        // Restart before expiry: the original fence/lease/TTL must still be present.
        let mut server = Running::start(&db, &dir, "http://127.0.0.1:1", &home.uri(), &[]).await;
        let row:(i64,OffsetDateTime,OffsetDateTime)=sqlx::query_as("SELECT j.token,j.lease_until,b.automatic_expires_at FROM post_relay_jobs j JOIN post_relay_bodies b USING(receipt) WHERE receipt='crashed-job'").fetch_one(&db.pool).await.unwrap();
        assert_eq!(
            row,
            (old.token, old.lease_until, expiry),
            "restart must not reclaim early or reset lease/72h TTL"
        );
        wait_no_jobs(&db).await;
        assert_eq!(
            home.received_requests().await.unwrap().len(),
            1,
            "one fresh permit after lease expiry"
        );
        let mut tx = db.database.get_transaction().await.unwrap();
        assert_eq!(
            db.database
                .post_relay_store()
                .renew_lease(tx.connection(), &old)
                .await
                .unwrap(),
            Recorded::Stale
        );
        tx.commit().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM post_relay_bodies")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "stale lease cannot resurrect successful body");
        server.term().await;
    }
}

// LD_PRELOAD interception exercises reqwest's real blocking resolver, not an async mock.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn post_relay_runtime_sigterm_bounds_blocking_dns_runtime_teardown() {
    let db = IsolatedDatabase::create().await;
    let dir = working_dir();
    let library = dir.path().join("b1-stall-dns.so");
    let compiled = std::process::Command::new("cc")
        .args(["-shared", "-fPIC", "-Wall", "-Wextra", "-Werror"])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/b1_stall_dns.c"
        ))
        .args(["-ldl", "-o"])
        .arg(&library)
        .output()
        .expect("Linux DNS regression requires a C compiler");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let home = MockServer::start().await;
    for stalled_http in [false, true] {
        let marker = dir.path().join(format!("dns-started-{stalled_http}"));
        let mut server = Running::start(
            &db,
            &dir,
            "http://b1-stalled-dns.invalid",
            &home.uri(),
            &[
                ("LD_PRELOAD", library.to_str().unwrap()),
                ("EMUMET_TEST_DNS_MARKER", marker.to_str().unwrap()),
                ("NO_PROXY", "*"),
                ("no_proxy", "*"),
                ("JWKS_TIMEOUT_MS", "200"),
                ("SERVER_SHUTDOWN_TIMEOUT_SECS", "2"),
                ("POST_RELAY_SENDER_ENABLED", "false"),
            ],
        )
        .await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while !marker.exists() {
                assert!(
                    server.child.try_wait().unwrap().is_none(),
                    "{}",
                    server.logs()
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the real blocking getaddrinfo must be entered");
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(
            server.logs().contains("JWKS refresh deadline exceeded")
                || server.logs().contains("JWKS refresh failed"),
            "request already timed out: {}",
            server.logs()
        );
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "started\n");
        // A public JSON-body route keeps HTTP drain pending without making another Ory call.
        // This must not grant runtime teardown a fresh 2s budget after async drain used it all.
        let _pending_body = if stalled_http {
            let mut socket =
                tokio::net::TcpStream::connect(server.base.trim_start_matches("http://"))
                    .await
                    .unwrap();
            socket.write_all(b"POST /oauth2/consent HTTP/1.1\r\nHost: emumet.example\r\nContent-Type: application/json\r\nContent-Length: 1000\r\n\r\n{").await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            Some(socket)
        } else {
            None
        };
        let started = Instant::now();
        server.signal("TERM");
        let status = tokio::time::timeout(Duration::from_secs(3), server.child.wait())
            .await
            .expect("2s shutdown must include runtime teardown, not wait for 20s libc DNS")
            .unwrap();
        assert!(status.success(), "{}", server.logs());
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            "started\n",
            "process exits before the blocking resolver finishes"
        );
        if stalled_http {
            assert!(
                server.logs().contains("Runtime drain deadline exceeded"),
                "HTTP actually exhausted the shared deadline: {}",
                server.logs()
            );
        }
        println!(
            "blocking-DNS SIGTERM stalled_http={stalled_http} exit elapsed: {:?}",
            started.elapsed()
        );
    }
}
