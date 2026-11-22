//! One fully wired world: real Ory, source-built Emumet, supplier, P1 receiver, signed remote peer.
use super::db::{eventually, Db};
use super::emumet::*;
use super::ory::{self, User};
use super::receiver::Receiver;
use super::stack::*;
use super::supplier::Supplier;
use serde_json::Value;
use std::time::Duration;

/// Public-IP literals: link registration only validates the origin (no probe, no DNS), and a
/// literal needs no resolver. Delivery is redirected to the loopback receiver by
/// `Db::point_link_at`, never by an SSRF exception in the build.
pub fn home(i: u8) -> String {
    format!("https://93.184.216.{}", 34 + i)
}
pub fn homes() -> Vec<String> {
    (0..8).map(home).collect()
}

pub struct World {
    pub db: Db,
    pub receiver: Receiver,
    /// A second, independent home (relink / old-home isolation cases).
    pub receiver2: Receiver,
    pub server: Emumet,
    pub supplier: Supplier,
    pub peer: std::sync::Arc<RemotePeer>,
    pub config: Config,
}

pub struct Linked {
    pub user: User,
    pub account: String,
    pub profile: String,
    pub profile_id: i64,
    pub link_id: String,
}

pub struct Options {
    pub sender: bool,
    pub port: Option<u16>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            sender: true,
            port: None,
        }
    }
}

impl World {
    pub async fn setup() -> Self {
        Self::setup_with(Options::default()).await
    }
    pub async fn setup_with(options: Options) -> Self {
        // TestModule::new moves the process cwd into a (later deleted) server directory.
        std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
        wait_ory().await;
        let db = Db::create().await;
        let supplier = Supplier::start(&db.url()).await;
        let receiver = Receiver::start(PUBLIC, "http://127.0.0.1:1").await;
        let mut config = Config::new(&db.url());
        config.sender = options.sender;
        config.port = options.port.or(Some(LOGIN_PORT));
        let receiver2 = Receiver::start(PUBLIC, "http://127.0.0.1:1").await;
        config.test_origins = vec![receiver.origin.clone(), receiver2.origin.clone()];
        config.allowed_origins = homes();
        let server = Emumet::start(&config).await;
        receiver.set_fetch_base(&server.base);
        receiver2.set_fetch_base(&server.base);
        Self {
            db,
            receiver,
            receiver2,
            server,
            supplier,
            peer: std::sync::Arc::new(RemotePeer::start().await),
            config,
        }
    }
    pub async fn user(&self, label: &str) -> User {
        ory::login_new_user(label).await
    }
    pub async fn linked(&self, label: &str, home_index: u8) -> Linked {
        let user = self.user(label).await;
        self.linked_for(user, label, home_index).await
    }
    pub async fn linked_for(&self, user: User, label: &str, home_index: u8) -> Linked {
        let username = format!(
            "t1_{label}_{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let (account, profile) = self.server.create_account(&user.jwt, &username).await;
        let link_id = self.register(&user, &profile, home_index).await;
        let profile_id = self.db.profile_id(&profile).await;
        Linked {
            user,
            account,
            profile,
            profile_id,
            link_id,
        }
    }
    /// Registers through the real API, then points delivery at the loopback receiver.
    pub async fn register(&self, user: &User, profile: &str, home_index: u8) -> String {
        let (status, link) = self
            .server
            .register_link(&user.jwt, profile, &home(home_index))
            .await;
        assert!(
            status == 201 || status == 200,
            "link registration: {status} {link}"
        );
        let id = link["id"].as_str().unwrap().to_string();
        self.db.point_link_at(&id, &self.receiver.origin).await;
        id
    }
    pub async fn register_at(
        &self,
        user: &User,
        profile: &str,
        home_index: u8,
        receiver: &Receiver,
    ) -> String {
        let (status, link) = self
            .server
            .register_link(&user.jwt, profile, &home(home_index))
            .await;
        assert!(
            status == 201 || status == 200,
            "link registration: {status} {link}"
        );
        let id = link["id"].as_str().unwrap().to_string();
        self.db.point_link_at(&id, &receiver.origin).await;
        id
    }
    /// Instance administrator: the Keto Instance#admins tuple of the user's AuthAccount.
    pub async fn make_admin(&self, user: &User) {
        let sub = ory::jwt_subject(&user.jwt);
        let id: i64 = sqlx::query_scalar("SELECT id FROM auth_accounts WHERE client_id=$1")
            .bind(&sub)
            .fetch_one(&self.db.iso.pool)
            .await
            .expect("auth account exists after the first API call");
        let r = reqwest::Client::new()
            .put(format!("{KETO_WRITE}/admin/relation-tuples"))
            .json(&serde_json::json!({"namespace":"Instance","object":"singleton","relation":"admins","subject_id":id.to_string()}))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "keto grant: {}", r.status());
    }
    /// Kills (SIGKILL) and starts the server again on the same DB/port; returns the start time.
    pub async fn restart(&mut self) -> Duration {
        self.server.kill().await;
        let started = std::time::Instant::now();
        self.server = Emumet::start(&self.config).await;
        let took = started.elapsed();
        self.receiver.set_fetch_base(&self.server.base);
        self.receiver2.set_fetch_base(&self.server.base);
        took
    }
    pub async fn receipts_gone(&self) {
        let db = &self.db;
        eventually("jobs drained", Duration::from_secs(30), || async {
            db.count("post_relay_jobs").await == 0
        })
        .await;
    }
    pub async fn envelope_for(&self, n: usize) -> Value {
        self.receiver
            .wait_accepted(n, Duration::from_secs(30))
            .await[n - 1]
            .envelope
            .clone()
    }
}

pub const LOGIN_PORT: u16 = 18080;
