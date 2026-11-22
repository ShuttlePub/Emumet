//! The source-built Emumet process under test, its HTTP API and the signed remote peer.
use super::stack::*;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};

pub const MASTER_PASSWORD: &[u8] = b"t1-e2e-only-master-password";

pub fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(20)).build().unwrap()
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

pub fn working_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let password = dir.path().join("master-key-password");
    std::fs::write(&password, MASTER_PASSWORD).unwrap();
    std::fs::set_permissions(password, std::fs::Permissions::from_mode(0o600)).unwrap();
    dir
}

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub sender: bool,
    pub inbox: bool,
    /// Loopback HTTP peers allowed ONLY by the test-mode build (EMUMET_TEST_RELAY_ORIGINS).
    pub test_origins: Vec<String>,
    /// Exact HTTPS origins of POST_RELAY_ALLOWED_ORIGINS (the production allowlist).
    pub allowed_origins: Vec<String>,
    /// Real Ory (default) or a custom URL for every Ory dependency.
    pub ory: Option<String>,
    pub port: Option<u16>,
    pub worker_id: u32,
    pub jwks_timeout_ms: u32,
    pub extra: Vec<(String, String)>,
}

impl Config {
    pub fn new(database_url: &str) -> Self {
        Self {
            database_url: database_url.into(),
            sender: true,
            inbox: true,
            test_origins: vec![],
            allowed_origins: vec![],
            ory: None,
            port: None,
            worker_id: 1,
            jwks_timeout_ms: 5000,
            extra: vec![],
        }
    }
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.extra.push((key.into(), value.into()));
        self
    }
}

pub struct Emumet {
    pub child: Child,
    pub base: String,
    pub address: String,
    pub log: std::path::PathBuf,
    pub dir: std::path::PathBuf,
    _keep: Option<tempfile::TempDir>,
}

impl Emumet {
    pub async fn start(config: &Config) -> Self {
        let dir = working_dir();
        let mut server = Self::start_in(config, dir.path()).await;
        server._keep = Some(dir);
        server
    }
    pub async fn start_in(config: &Config, dir: &std::path::Path) -> Self {
        let port = config.port.unwrap_or_else(free_port);
        let address = format!("127.0.0.1:{port}");
        let log = dir.join(format!("server-{port}-{}.log", uuid::Uuid::new_v4().simple()));
        let output = std::fs::File::create(&log).unwrap();
        let (hydra_issuer, hydra_admin, kratos, keto_r, keto_w) = match &config.ory {
            Some(u) => (u.clone(), u.clone(), u.clone(), u.clone(), u.clone()),
            None => (
                format!("{HYDRA_PUBLIC}/"),
                HYDRA_ADMIN.to_string(),
                KRATOS_PUBLIC.to_string(),
                KETO_READ.to_string(),
                KETO_WRITE.to_string(),
            ),
        };
        let mut command = Command::new(env!("CARGO_BIN_EXE_server"));
        command
            .current_dir(dir)
            .kill_on_drop(true)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("LD_LIBRARY_PATH", std::env::var("LD_LIBRARY_PATH").unwrap_or_default())
            .env("DATABASE_URL", &config.database_url)
            .env("PUBLIC_BASE_URL", PUBLIC)
            .env("HTTP_LISTEN_ADDR", &address)
            .env("HYDRA_ISSUER_URL", hydra_issuer)
            .env("HYDRA_ADMIN_URL", hydra_admin)
            .env("KRATOS_PUBLIC_URL", kratos)
            .env("KETO_READ_URL", keto_r)
            .env("KETO_WRITE_URL", keto_w)
            .env("EXPECTED_AUDIENCE", "account")
            .env("WORKER_ID", config.worker_id.to_string())
            .env("AWS_EC2_METADATA_DISABLED", "true")
            .env("EMUMET_FILE_LOG_ENABLED", "false")
            .env("EMUMET_TEST_MODE_TOKEN", "t1-e2e-test-token")
            .env("AP_TEST_ALLOWED_FETCH_HOSTS", "127.0.0.1")
            .env("EMUMET_TEST_RELAY_ORIGINS", serde_json::to_string(&config.test_origins).unwrap())
            .env("POST_RELAY_ALLOWED_ORIGINS", serde_json::to_string(&config.allowed_origins).unwrap())
            .env("POST_RELAY_SENDER_ENABLED", config.sender.to_string())
            .env("POST_RELAY_INBOX_ENABLED", config.inbox.to_string())
            .env("JWKS_TIMEOUT_MS", config.jwks_timeout_ms.to_string())
            .env("SERVER_SHUTDOWN_TIMEOUT_SECS", "5")
            .env("CORS_ALLOWED_ORIGINS", "*")
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(output.try_clone().unwrap())
            .stderr(output);
        for (k, v) in &config.extra {
            command.env(k, v);
        }
        let child = command.spawn().expect("spawn server");
        let mut server = Self { child, base: format!("http://{address}"), address, log, dir: dir.to_path_buf(), _keep: None };
        let c = client();
        let started = Instant::now();
        loop {
            assert!(server.child.try_wait().unwrap().is_none(), "server exited early: {}", server.logs());
            if c.get(format!("{}/healthz", server.base)).send().await.is_ok_and(|r| r.status().is_success()) {
                return server;
            }
            assert!(started.elapsed() < Duration::from_secs(180), "server start bound exceeded: {}", server.logs());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    pub fn logs(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
    pub fn pid(&self) -> u32 {
        self.child.id().expect("running")
    }
    pub fn signal(&self, signal: &str) {
        assert!(std::process::Command::new("kill").arg(format!("-{signal}")).arg(self.pid().to_string()).status().unwrap().success());
    }
    pub async fn term(&mut self) {
        self.signal("TERM");
        let status = tokio::time::timeout(Duration::from_secs(20), self.child.wait()).await.expect("SIGTERM exit bound").unwrap();
        assert!(status.success(), "{}", self.logs());
    }
    pub async fn kill(&mut self) {
        self.child.kill().await.unwrap();
        let _ = self.child.wait().await;
    }
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    // ---- authenticated API -------------------------------------------------------------
    pub async fn api(&self, method: &str, path: &str, jwt: &str, body: Option<Value>) -> (u16, Value) {
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut request = client().request(method, self.url(path)).bearer_auth(jwt);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
    /// Returns (account_nanoid, profile_nanoid).
    pub async fn create_account(&self, jwt: &str, username: &str) -> (String, String) {
        let (status, body) = self
            .api("POST", "/api/v1/accounts", jwt, Some(json!({"name": format!("T1 {username}"), "is_bot": false, "profile_username": username})))
            .await;
        assert_eq!(status, 201, "account creation: {body}");
        (body["id"].as_str().unwrap().into(), body["profile_id"].as_str().unwrap().into())
    }
    pub async fn register_link(&self, jwt: &str, profile: &str, origin: &str) -> (u16, Value) {
        self.api("POST", &format!("/api/v1/profiles/{profile}/links"), jwt, Some(json!({"origin": origin, "trust_mode": 3}))).await
    }
    pub async fn unlink(&self, jwt: &str, profile: &str, link: &str) -> (u16, Value) {
        self.api("DELETE", &format!("/api/v1/profiles/{profile}/links/{link}"), jwt, None).await
    }
    pub async fn links(&self, jwt: &str, profile: &str) -> Value {
        self.api("GET", &format!("/api/v1/profiles/{profile}/links"), jwt, None).await.1
    }
}

// ---- the remote ActivityPub peer that signs inbound requests ---------------------------------
pub struct RemotePeer {
    pub server: MockServer,
    pub actor: String,
    pub key_id: String,
    private_pem: Vec<u8>,
}

pub struct Inbound<'a> {
    pub body: Vec<u8>,
    pub content_type: &'a str,
    /// Extra headers included in the signature (e.g. big ones to overflow the evidence budget).
    pub signed_headers: Vec<(String, String)>,
    /// Extra, unsigned headers (e.g. Cookie / Authorization: must never be stored).
    pub plain_headers: Vec<(String, String)>,
}

impl<'a> Inbound<'a> {
    pub fn new(body: impl Into<Vec<u8>>) -> Self {
        Self { body: body.into(), content_type: "application/activity+json", signed_headers: vec![], plain_headers: vec![] }
    }
}

impl RemotePeer {
    pub async fn start() -> Self {
        use driver::crypto::Rsa2048RawGenerator;
        use kernel::interfaces::crypto::RawKeyGenerator;
        let keys = Rsa2048RawGenerator.generate_raw().unwrap();
        let server = MockServer::start().await;
        let actor = format!("{}/actor", server.uri());
        let key_id = format!("{actor}#key");
        Mock::given(path("/actor"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": actor, "type": "Person", "inbox": format!("{}/inbox", server.uri()),
                "publicKey": {"id": key_id, "owner": actor, "publicKeyPem": keys.public_key_pem}
            })))
            .mount(&server)
            .await;
        Self { server, actor, key_id, private_pem: keys.private_key_pem.to_vec() }
    }
    pub fn activity_id(&self) -> String {
        format!("{}/activities/{}", self.server.uri(), uuid::Uuid::new_v4())
    }
    pub fn activity(&self, kind: &str, object: Value) -> Value {
        json!({"type": kind, "id": self.activity_id(), "actor": self.actor, "object": object})
    }
    pub fn note(&self, text: &str) -> Value {
        self.activity("Create", json!({"type": "Note", "content": text}))
    }
    pub async fn post(&self, server: &Emumet, profile_nanoid: &str, inbound: Inbound<'_>) -> reqwest::Response {
        use driver::http_signing::HttpSignerImpl;
        use kernel::interfaces::crypto::SigningAlgorithm;
        use kernel::interfaces::http_signing::{HttpSigner, HttpSigningRequest};
        use sha2::Digest;
        let target = format!("/ap/profiles/{profile_nanoid}/inbox");
        let digest = format!(
            "SHA-256={}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, sha2::Sha256::digest(&inbound.body))
        );
        let mut headers = std::collections::HashMap::from([
            ("host".to_string(), "emumet.e2e.example".to_string()),
            ("date".to_string(), httpdate::fmt_http_date(std::time::SystemTime::now())),
            ("digest".to_string(), digest),
        ]);
        for (k, v) in &inbound.signed_headers {
            headers.insert(k.to_ascii_lowercase(), v.clone());
        }
        let extra_names: Vec<String> = inbound.signed_headers.iter().map(|(k, _)| k.to_ascii_lowercase()).collect();
        let cavage_headers = if extra_names.is_empty() {
            HttpSignerImpl
            .sign(
                &HttpSigningRequest { method: "POST".into(), url: format!("{PUBLIC}{target}"), headers: headers.clone(), body: Some(inbound.body.clone()) },
                &self.private_pem,
                &self.key_id,
                &SigningAlgorithm::Rsa2048,
            )
            .await
            .unwrap()
            .cavage_headers
        } else {
            // The shipped signer only covers (request-target) host date digest; extra covered
            // headers (to overflow the evidence budget) need a hand-built Cavage signature.
            use rsa::pkcs1::DecodeRsaPrivateKey;
            use rsa::pkcs8::DecodePrivateKey;
            use rsa::signature::{SignatureEncoding, Signer};
            let pem = String::from_utf8(self.private_pem.clone()).unwrap();
            let key = rsa::RsaPrivateKey::from_pkcs8_pem(&pem).or_else(|_| rsa::RsaPrivateKey::from_pkcs1_pem(&pem)).unwrap();
            let mut names = vec!["(request-target)".to_string(), "host".into(), "date".into(), "digest".into()];
            names.extend(extra_names.clone());
            let lines: Vec<String> = names
                .iter()
                .map(|n| if n == "(request-target)" { format!("(request-target): post {target}") } else { format!("{n}: {}", headers[n]) })
                .collect();
            let signature = rsa::pkcs1v15::SigningKey::<sha2::Sha256>::new(key).sign(lines.join("\n").as_bytes());
            let header = format!(
                "keyId=\"{}\",algorithm=\"rsa-sha256\",headers=\"{}\",signature=\"{}\"",
                self.key_id,
                names.join(" "),
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, signature.to_bytes())
            );
            std::collections::HashMap::from([("signature".to_string(), header)])
        };
        let mut request = client().post(server.url(&target)).header("content-type", inbound.content_type).body(inbound.body.clone());
        for (k, v) in headers.into_iter().chain(cavage_headers) {
            request = request.header(k, v);
        }
        for (k, v) in inbound.plain_headers {
            request = request.header(k, v);
        }
        request.send().await.expect("inbox request failed")
    }
    pub async fn deliver_json(&self, server: &Emumet, profile_nanoid: &str, activity: &Value) -> reqwest::Response {
        self.post(server, profile_nanoid, Inbound::new(serde_json::to_vec(activity).unwrap())).await
    }
}
