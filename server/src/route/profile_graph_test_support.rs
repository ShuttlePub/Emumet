//! Shared fixtures of the Profile graph tests (P3i): an isolated database that carries the P2
//! schema fixture, an `AppModule` on it, local Profiles that are actors, and signed requests from
//! remote peers.
//!
//! The P2 tables are not part of the production migrations, so these tests never run on the shared
//! `DATABASE_URL` database: every world creates its own database (production migrations + the P2
//! fixture) and drops it afterwards.

use crate::auth::{
    auth_middleware, encode_test_jwt, generate_test_keys, AuthClaims, JwksCache, OidcConfig,
};
use crate::handler::AppModule;
use application::dto::account::CreateAccountDto;
use application::service::account::CreateAccountUseCase;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::crypto::SigningAlgorithm;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::http_signing::{HttpSigner, HttpSigningRequest};
use kernel::interfaces::repository::{
    BeginFollowOutcome, DependOnProfileFollowRepository, ProfileFollowRepository,
};
use kernel::prelude::entity::{
    AuthAccountId, GraphActivityId, ProfileFollow, ProfileFollowId, ProfileGraphActor, ProfileId,
};
use serde_json::{json, Value};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub(crate) const AUDIENCE: &str = "emumet";

static DATABASE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A throw-away database with all production migrations applied.
pub(crate) struct IsolatedDatabase {
    pub pool: PgPool,
    name: String,
    admin_options: PgConnectOptions,
}

impl IsolatedDatabase {
    pub async fn create() -> Self {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set for the Profile graph database tests");
        let admin_options = PgConnectOptions::from_str(&url).unwrap();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!(
            "emumet_p3i_{}_{}_{}",
            std::process::id(),
            DATABASE_COUNTER.fetch_add(1, Ordering::Relaxed),
            nanos % 1_000_000_000
        );
        let mut admin = PgConnection::connect_with(&admin_options).await.unwrap();
        admin
            .execute(format!("CREATE DATABASE \"{name}\"").as_str())
            .await
            .unwrap();
        admin.close().await.unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect_with(admin_options.clone().database(&name))
            .await
            .unwrap();
        sqlx::migrate!("../migrations").run(&pool).await.unwrap();
        Self {
            pool,
            name,
            admin_options,
        }
    }
}

impl Drop for IsolatedDatabase {
    fn drop(&mut self) {
        let name = self.name.clone();
        let options = self.admin_options.clone();
        let _ = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                if let Ok(mut admin) = PgConnection::connect_with(&options).await {
                    let _ = admin
                        .execute(
                            format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)").as_str(),
                        )
                        .await;
                }
            });
        })
        .join();
    }
}

fn ensure_master_key_password_file() {
    use std::io::Write;

    let target = std::path::Path::new("./master-key-password");
    if target.exists() {
        return;
    }
    let temporary = std::path::PathBuf::from(format!(
        "./master-key-password.{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).expect("create temporary password");
    file.write_all(b"develop\n").expect("write password");
    drop(file);
    let linked = std::fs::hard_link(&temporary, target);
    std::fs::remove_file(&temporary).expect("remove temporary password");
    if let Err(error) = linked {
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists, "{error}");
    }
}

/// A local Profile that is an actor (username and active key).
#[derive(Clone)]
pub(crate) struct LocalProfile {
    pub id: ProfileId,
    pub nanoid: String,
    pub actor_url: String,
}

/// An authenticated user (bearer token) of the world.
pub(crate) struct User {
    pub token: String,
    pub auth_account_id: AuthAccountId,
}

pub(crate) struct GraphWorld {
    pub database: IsolatedDatabase,
    pub module: AppModule,
    pub base_url: String,
    pub host_header: String,
    issuer: String,
    keys: crate::auth::TestKeys,
    host_id: i64,
    oidc_config: Arc<OidcConfig>,
    jwks_cache: Arc<JwksCache>,
    _keto: MockServer,
}

impl GraphWorld {
    pub async fn new() -> Self {
        ensure_master_key_password_file();
        kernel::ensure_generator_initialized();
        let database = IsolatedDatabase::create().await;
        let keto = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/admin/relation-tuples"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&keto)
            .await;
        let module = AppModule::new_for_test_database(
            driver::database::PostgresDatabase::from_pool(database.pool.clone()),
            "http://localhost:65535".to_string(),
            "http://localhost:65535".to_string(),
            keto.uri(),
            keto.uri(),
        )
        .await
        .expect("AppModule init on the isolated database");
        let base_url = module
            .public_base_url()
            .as_str()
            .trim_end_matches('/')
            .to_string();
        let parsed = url::Url::parse(&base_url).unwrap();
        let host_header = match parsed.port() {
            Some(port) => format!("{}:{port}", parsed.host_str().unwrap()),
            None => parsed.host_str().unwrap().to_string(),
        };
        let issuer = format!("https://issuer.example/{}", uuid::Uuid::new_v4());
        let keys = generate_test_keys();
        let host_id = kernel::generate_id();
        sqlx::query("INSERT INTO auth_hosts (id, url) VALUES ($1, $2)")
            .bind(host_id)
            .bind(&issuer)
            .execute(&database.pool)
            .await
            .unwrap();
        let oidc_config = Arc::new(OidcConfig {
            issuer_url: issuer.clone(),
            expected_audience: AUDIENCE.to_string(),
            jwks_refetch_interval_secs: 0,
        });
        let jwks_cache = Arc::new(JwksCache::new_with_jwks(
            issuer.clone(),
            keys.jwk_set.clone(),
        ));
        Self {
            database,
            module,
            base_url,
            host_header,
            issuer,
            keys,
            host_id,
            oidc_config,
            jwks_cache,
            _keto: keto,
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.database.pool
    }

    /// Router with the Profile inbox mounted exactly the way P4 will mount it.
    pub fn inbox_router(&self) -> axum::Router {
        use crate::route::activitypub::profile_inbox::ProfileInboxRouter;
        axum::Router::new()
            .nest("/ap", axum::Router::new().route_profile_inbox())
            .with_state(self.module.clone())
    }

    /// Router with the followers / following reads behind the real auth middleware.
    pub fn follow_list_router(&self) -> axum::Router {
        use crate::route::profile_follow_list::ProfileFollowListRouter;
        axum::Router::new()
            .nest("/api/v1", axum::Router::new().route_profile_follow_lists())
            .layer(axum::middleware::from_fn_with_state(
                self.module.clone(),
                crate::api::organization_context::organization_context_middleware,
            ))
            .layer(axum::middleware::from_fn_with_state(
                (self.oidc_config.clone(), self.jwks_cache.clone()),
                auth_middleware,
            ))
            .with_state(self.module.clone())
    }

    /// The production root router (the function `main` uses) on this world.
    pub fn root_router(&self) -> axum::Router {
        crate::root_router::build_root_router(
            self.module.clone(),
            self.oidc_config.clone(),
            self.jwks_cache.clone(),
        )
    }

    pub async fn new_user(&self) -> User {
        let auth_account_id = AuthAccountId::default();
        let client_id = format!("client-{}", uuid::Uuid::new_v4());
        sqlx::query("INSERT INTO auth_accounts (id, host_id, client_id) VALUES ($1, $2, $3)")
            .bind(auth_account_id.as_ref())
            .bind(self.host_id)
            .bind(&client_id)
            .execute(self.pool())
            .await
            .unwrap();
        let claims = AuthClaims {
            iss: self.issuer.clone(),
            sub: client_id,
            aud: crate::auth::OneOrMany::One(AUDIENCE.to_string()),
            exp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock after Unix epoch")
                .as_secs()
                + 3600,
        };
        User {
            token: encode_test_jwt(&claims, &self.keys.encoding_key, &self.keys.kid),
            auth_account_id,
        }
    }

    /// A Profile of a new personal Account of `user`. With `actor` it has a username and an
    /// active signing key (an actor, as every Profile created through the API), otherwise
    /// neither (a stripped, pre-cutover shaped Profile).
    pub async fn create_profile(&self, user: &User, actor: bool) -> LocalProfile {
        let name = kernel::test_utils::unique_account_name()
            .as_ref()
            .to_string();
        let username = format!("u{}", uuid::Uuid::new_v4().simple());
        self.module
            .create_account_with_profile_username(
                user.auth_account_id.clone(),
                CreateAccountDto {
                    name: name.clone(),
                    is_bot: false,
                },
                username,
            )
            .await
            .unwrap();
        let (id, nanoid): (i64, String) = sqlx::query_as(
            "SELECT p.id, p.nanoid FROM profiles p JOIN accounts a ON a.id = p.account_id WHERE a.name = $1",
        )
        .bind(&name)
        .fetch_one(self.pool())
        .await
        .unwrap();
        let profile = LocalProfile {
            id: ProfileId::new(id),
            actor_url: format!("{}/ap/profiles/{nanoid}", self.base_url),
            nanoid,
        };
        if !actor {
            // Creation always gives a Profile its username and key; tests that need a Profile
            // that is not an actor (a pre-cutover Profile) strip both.
            sqlx::query("DELETE FROM profile_signing_keys WHERE profile_id = $1")
                .bind(profile.id.as_ref())
                .execute(self.pool())
                .await
                .unwrap();
            sqlx::query("UPDATE profiles SET username = NULL WHERE id = $1")
                .bind(profile.id.as_ref())
                .execute(self.pool())
                .await
                .unwrap();
        }
        profile
    }

    /// An actor Profile owned by a fresh user.
    pub async fn actor_profile(&self) -> LocalProfile {
        let user = self.new_user().await;
        self.create_profile(&user, true).await
    }

    /// Stores a follow generation through the P2 repository (as P3o / P3i do).
    pub async fn seed_follow(
        &self,
        follower: ProfileGraphActor,
        followee: ProfileGraphActor,
        activity_id: &str,
        accepted: bool,
    ) {
        let id = ProfileFollowId::new(kernel::generate_id());
        let activity_id = GraphActivityId::new(activity_id).unwrap();
        let now = OffsetDateTime::now_utc();
        let follow = if accepted {
            ProfileFollow::new_accepted(id, follower, followee, activity_id, now)
        } else {
            ProfileFollow::new_pending(id, follower, followee, activity_id, now)
        }
        .unwrap();
        let mut conn = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        let outcome = self
            .module
            .profile_follow_repository()
            .begin_generation(&mut conn, &follow)
            .await
            .unwrap();
        assert_eq!(outcome, BeginFollowOutcome::Created);
    }
}

struct KeyPair {
    private_pem: Vec<u8>,
    public_pem: String,
}

fn generate_key_pair() -> KeyPair {
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use rsa::{RsaPrivateKey, RsaPublicKey};

    let private_key = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public_key = RsaPublicKey::from(&private_key);
    KeyPair {
        private_pem: private_key
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .to_string()
            .into_bytes(),
        public_pem: public_key.to_public_key_pem(LineEnding::LF).unwrap(),
    }
}

/// Key generation is slow in debug builds, so the peers share two key pairs; what makes a peer
/// distinct is its actor URL and key id.
static PEER_KEYS: LazyLock<KeyPair> = LazyLock::new(generate_key_pair);
static STRANGER_KEYS: LazyLock<KeyPair> = LazyLock::new(generate_key_pair);

/// A remote ActivityPub actor whose key and inbox are known to the server without network access.
#[derive(Clone)]
pub(crate) struct RemotePeer {
    pub actor_url: String,
    pub key_id: String,
    pub inbox_url: String,
}

pub(crate) fn peer_private_pem() -> &'static [u8] {
    &PEER_KEYS.private_pem
}

impl RemotePeer {
    /// Registered peer: its key verifies, and its actor document (inbox) resolves.
    pub fn new() -> Self {
        Self::at(&format!(
            "https://peer-{}.remote.example/users/peer",
            uuid::Uuid::new_v4().simple()
        ))
    }

    /// Registered peer with a chosen actor URL.
    pub fn at(actor_url: &str) -> Self {
        let peer = Self::unresolved_at(actor_url);
        peer.register_actor_document();
        peer
    }

    /// Peer whose key verifies but whose actor document cannot be resolved (no inbox known).
    pub fn unresolved() -> Self {
        Self::unresolved_at(&format!(
            "https://peer-{}.remote.example/users/peer",
            uuid::Uuid::new_v4().simple()
        ))
    }

    fn unresolved_at(actor_url: &str) -> Self {
        let peer = Self {
            key_id: format!("{actor_url}#main-key"),
            inbox_url: format!("{actor_url}/inbox"),
            actor_url: actor_url.to_string(),
        };
        driver::http_signing::inject_test_actor_key(
            &peer.key_id,
            PEER_KEYS.public_pem.clone(),
            &peer.actor_url,
        );
        peer
    }

    pub fn register_actor_document(&self) {
        application::service::activitypub::inject_test_remote_actor(
            &self.actor_url,
            &self.inbox_url,
        );
    }

    pub fn activity_id(&self, label: &str) -> String {
        format!("{}/activities/{label}", self.actor_url)
    }

    pub fn follow(&self, id: &str, target: &str) -> Value {
        json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": id,
            "type": "Follow",
            "actor": self.actor_url,
            "object": target,
        })
    }

    pub fn undo(&self, id: &str, object: Value) -> Value {
        json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": id,
            "type": "Undo",
            "actor": self.actor_url,
            "object": object,
        })
    }

    /// Accept / Reject of a Follow sent by `follower_actor` to this peer.
    pub fn respond(&self, kind: &str, id: &str, follow: Value) -> Value {
        json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": id,
            "type": kind,
            "actor": self.actor_url,
            "object": follow,
        })
    }

    pub async fn post(&self, world: &GraphWorld, profile_nanoid: &str, body: &Value) -> StatusCode {
        self.post_as(
            world,
            profile_nanoid,
            body,
            &PEER_KEYS.private_pem,
            &self.key_id,
        )
        .await
    }

    /// Signed with a key that does not belong to the peer.
    pub async fn post_with_stranger_key(
        &self,
        world: &GraphWorld,
        profile_nanoid: &str,
        body: &Value,
    ) -> StatusCode {
        self.post_as(
            world,
            profile_nanoid,
            body,
            &STRANGER_KEYS.private_pem,
            &self.key_id,
        )
        .await
    }

    pub async fn post_as(
        &self,
        world: &GraphWorld,
        profile_nanoid: &str,
        body: &Value,
        private_pem: &[u8],
        key_id: &str,
    ) -> StatusCode {
        let body = serde_json::to_vec(body).unwrap();
        self.post_raw(
            world,
            profile_nanoid,
            body,
            Some((private_pem, key_id)),
            None,
        )
        .await
    }

    /// `signing` = `None` sends an unsigned request. `tampered_body` replaces the body after the
    /// signature was computed.
    pub async fn post_raw(
        &self,
        world: &GraphWorld,
        profile_nanoid: &str,
        body: Vec<u8>,
        signing: Option<(&[u8], &str)>,
        tampered_body: Option<Vec<u8>>,
    ) -> StatusCode {
        let base = world.base_url.clone();
        let host_header = world.host_header.clone();
        let path = format!("/ap/profiles/{profile_nanoid}/inbox");
        let url = format!("{base}{path}");
        let mut headers: HashMap<String, String> = HashMap::new();
        headers.insert(
            "content-type".to_string(),
            "application/activity+json".to_string(),
        );
        if let Some((private_pem, key_id)) = signing {
            let date = httpdate::fmt_http_date(SystemTime::now());
            let digest = {
                use sha2::Digest;
                let hash = sha2::Sha256::digest(&body);
                format!(
                    "SHA-256={}",
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, hash)
                )
            };
            let mut to_sign = HashMap::new();
            to_sign.insert("host".to_string(), host_header.clone());
            to_sign.insert("date".to_string(), date.clone());
            to_sign.insert("digest".to_string(), digest.clone());
            to_sign.insert(
                "content-type".to_string(),
                "application/activity+json".to_string(),
            );
            let signed = driver::http_signing::HttpSignerImpl
                .sign(
                    &HttpSigningRequest {
                        method: "POST".to_string(),
                        url,
                        headers: to_sign,
                        body: Some(body.clone()),
                    },
                    private_pem,
                    key_id,
                    &SigningAlgorithm::Rsa2048,
                )
                .await
                .expect("signing failed");
            headers.extend(signed.cavage_headers);
            headers.insert("host".to_string(), host_header);
            headers.insert("date".to_string(), date);
            headers.insert("digest".to_string(), digest);
        } else {
            headers.insert("host".to_string(), host_header);
        }
        let mut builder = Request::builder().method("POST").uri(&path);
        for (name, value) in &headers {
            builder = builder.header(name, value);
        }
        let response = world
            .inbox_router()
            .oneshot(
                builder
                    .body(Body::from(tampered_body.unwrap_or(body)))
                    .unwrap(),
            )
            .await
            .unwrap();
        response.status()
    }
}
