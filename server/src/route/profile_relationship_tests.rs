//! Real-database tests for the Profile relationship routes (P3o), run against an isolated database
//! that has the P2 schema fixture applied, with a fake peer (HTTP mock).

use super::ProfileRelationshipRouter;
use crate::api::profile_delivery_worker::spawn_profile_delivery_worker;
use crate::auth::{
    auth_middleware, encode_test_jwt, generate_test_keys, AuthClaims, JwksCache, OidcConfig,
};
use crate::handler::AppModule;
use crate::route::profile_follow_list::ProfileFollowListRouter;
use application::profile_signing_key::CreateProfileSigningKeyUseCase;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use driver::crypto::FilePasswordProvider;
use driver::database::isolated_db::IsolatedDatabase;
use http_body_util::BodyExt;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::crypto::SigningAlgorithm;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnDatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{
    AuthAccountRepository, AuthHostRepository, DependOnAuthAccountRepository,
    DependOnAuthHostRepository,
};
use kernel::prelude::entity::{
    Account, AccountKind, AuthAccountId, AuthHostId, Nanoid, Profile, ProfileDeliveryPolicy,
    ProfileId,
};
use kernel::test_utils::{
    unique_account_name, AccountBuilder, AuthAccountBuilder, AuthHostBuilder, ProfileBuilder,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const AUDIENCE: &str = "emumet";

struct User {
    token: String,
    account: Account,
    profile: Profile,
}

struct Harness {
    iso: IsolatedDatabase,
    module: AppModule,
    router: axum::Router,
    issuer: String,
    host_id: AuthHostId,
    keys: crate::auth::TestKeys,
    peer: MockServer,
    password: tempfile::NamedTempFile,
}

impl Harness {
    async fn new() -> Self {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create().await;
        let password = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(password.path(), b"relationship-route-password").unwrap();
        let issuer = format!("https://issuer.example/{}", uuid::Uuid::new_v4());
        let module = AppModule::new_for_oauth2_test(
            "http://localhost:65535".to_string(),
            "http://localhost:65535".to_string(),
        )
        .await
        .expect("AppModule init failed (is DATABASE_URL set?)")
        .with_database(iso.database.clone())
        .with_loopback_profile_delivery()
        .with_password_provider(FilePasswordProvider::with_paths(
            password.path(),
            password.path(),
        ));
        let keys = generate_test_keys();
        let host_id = AuthHostId::default();
        let auth_host = AuthHostBuilder::new()
            .id(host_id.clone())
            .url(issuer.clone())
            .build();
        let mut executor = module.database_connection().connection().await.unwrap();
        module
            .auth_host_repository()
            .create(&mut executor, &auth_host)
            .await
            .expect("seed auth host");
        drop(executor);

        let oidc_config = Arc::new(OidcConfig {
            issuer_url: issuer.clone(),
            expected_audience: AUDIENCE.to_string(),
            jwks_refetch_interval_secs: 0,
        });
        let jwks_cache = Arc::new(JwksCache::new_with_jwks(
            issuer.clone(),
            keys.jwk_set.clone(),
        ));
        let router = axum::Router::new()
            // Both mount functions on one router: the paths must not overlap (axum panics on a
            // duplicate method registration for a path).
            .nest(
                "/api/v1",
                axum::Router::new()
                    .route_profile_relationships()
                    .route_profile_follow_lists(),
            )
            .layer(axum::middleware::from_fn_with_state(
                module.clone(),
                crate::api::organization_context::organization_context_middleware,
            ))
            .layer(axum::middleware::from_fn_with_state(
                (oidc_config, jwks_cache),
                auth_middleware,
            ))
            .with_state(module.clone());
        Self {
            iso,
            module,
            router,
            issuer,
            host_id,
            keys,
            peer: MockServer::start().await,
            password,
        }
    }

    fn token_for(&self, subject: &str) -> String {
        let claims = AuthClaims {
            iss: self.issuer.clone(),
            sub: subject.to_string(),
            aud: crate::auth::OneOrMany::One(AUDIENCE.to_string()),
            exp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock after Unix epoch")
                .as_secs()
                + 3600,
        };
        encode_test_jwt(&claims, &self.keys.encoding_key, &self.keys.kid)
    }

    async fn seed_account(&self, kind: AccountKind) -> Account {
        let account = AccountBuilder::new()
            .kind(kind)
            .name(unique_account_name())
            .build();
        let mut executor = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        self.module
            .account_read_model()
            .create(&mut executor, &account)
            .await
            .expect("seed account");
        account
    }

    async fn seed_profile(&self, account: &Account) -> Profile {
        let profile = ProfileBuilder::new()
            .id(ProfileId::new(kernel::generate_id()))
            .account_id(account.id().clone())
            .nanoid(Nanoid::default())
            .build();
        let mut executor = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        self.module
            .profile_read_model()
            .create(&mut executor, &profile)
            .await
            .expect("seed profile");
        profile
    }

    async fn user(&self) -> User {
        let subject = format!("subject-{}", uuid::Uuid::new_v4());
        let auth_account_id = AuthAccountId::default();
        let auth_account = AuthAccountBuilder::new()
            .id(auth_account_id.clone())
            .host(self.host_id.clone())
            .client_id(subject.clone())
            .build();
        let account = self.seed_account(AccountKind::Personal).await;
        {
            let mut executor = self
                .module
                .database_connection()
                .connection()
                .await
                .unwrap();
            self.module
                .auth_account_repository()
                .create(&mut executor, &auth_account)
                .await
                .expect("seed auth account");
            self.module
                .account_read_model()
                .link_auth_account(&mut executor, account.id(), &auth_account_id)
                .await
                .expect("link auth account");
        }
        let profile = self.seed_profile(&account).await;
        User {
            token: self.token_for(&subject),
            account,
            profile,
        }
    }

    async fn call(
        &self,
        http_method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(http_method).uri(uri);
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let request = match body {
            Some(body) => builder
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => builder.body(Body::empty()),
        }
        .expect("valid request");
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn mount_actor(&self, actor_path: &str) -> String {
        let id = format!("{}{actor_path}", self.peer.uri());
        Mock::given(method("GET"))
            .and(path(actor_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": id,
                "type": "Person",
                "inbox": format!("{}/inbox", self.peer.uri()),
            })))
            .mount(&self.peer)
            .await;
        id
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar::<_, i64>(sql)
            .fetch_one(&self.iso.pool)
            .await
            .unwrap()
    }
}

fn uri(profile: &Profile, tail: &str) -> String {
    format!("/api/v1/profiles/{}/{tail}", profile.nanoid().as_ref())
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn follow_list_and_unfollow_round_trip_through_the_routes() {
    let h = Harness::new().await;
    let alice = h.user().await;
    let bob = h.mount_actor("/users/bob").await;

    let (status, created) = h
        .call(
            Method::POST,
            &uri(&alice.profile, "follows"),
            Some(&alice.token),
            Some(json!({"target": bob})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(
        created["profile_id"],
        alice.profile.nanoid().as_ref().as_str()
    );
    assert_eq!(created["target"], bob);
    assert_eq!(created["state"], "pending");
    let follow_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(
        h.count("SELECT count(*) FROM profile_delivery_jobs").await,
        1
    );

    let (status, again) = h
        .call(
            Method::POST,
            &uri(&alice.profile, "follows"),
            Some(&alice.token),
            Some(json!({"target": bob})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");

    let (status, list) = h
        .call(
            Method::GET,
            &uri(&alice.profile, "following"),
            Some(&alice.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["id"], follow_id);

    let (status, _) = h
        .call(
            Method::DELETE,
            &uri(&alice.profile, &format!("follows/{follow_id}")),
            Some(&alice.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, list) = h
        .call(
            Method::GET,
            &uri(&alice.profile, "following"),
            Some(&alice.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(list["items"].as_array().unwrap().is_empty());
    let (status, _) = h
        .call(
            Method::DELETE,
            &uri(&alice.profile, &format!("follows/{follow_id}")),
            Some(&alice.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn relationship_routes_authenticate_authorize_and_validate() {
    let h = Harness::new().await;
    let alice = h.user().await;
    let mallory = h.user().await;
    let bob = h.mount_actor("/users/bob").await;
    let follows = uri(&alice.profile, "follows");

    let (status, _) = h
        .call(Method::POST, &follows, None, Some(json!({"target": bob})))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = h
        .call(
            Method::POST,
            &follows,
            Some(&mallory.token),
            Some(json!({"target": bob})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h
        .call(
            Method::POST,
            "/api/v1/profiles/no-such-profile/follows",
            Some(&alice.token),
            Some(json!({"target": bob})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = h
        .call(
            Method::POST,
            &follows,
            Some(&alice.token),
            Some(json!({"target": "ftp://nope"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = h
        .call(
            Method::GET,
            &uri(&alice.profile, "following"),
            Some(&mallory.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(h.count("SELECT count(*) FROM profile_follows").await, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn block_and_mute_routes_apply_the_individual_actor_semantics() {
    let h = Harness::new().await;
    let alice = h.user().await;
    let bob = h.mount_actor("/users/bob").await;

    let (status, mute) = h
        .call(
            Method::POST,
            &uri(&alice.profile, "mutes"),
            Some(&alice.token),
            Some(json!({"target": bob})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{mute}");
    assert_eq!(mute["kind"], "mute");
    let (status, _) = h
        .call(
            Method::POST,
            &uri(&alice.profile, "mutes"),
            Some(&alice.token),
            Some(json!({"target": bob})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = h
        .call(
            Method::POST,
            &uri(&alice.profile, "follows"),
            Some(&alice.token),
            Some(json!({"target": bob})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a mute does not reject follows"
    );

    let (status, block) = h
        .call(
            Method::POST,
            &uri(&alice.profile, "blocks"),
            Some(&alice.token),
            Some(json!({"target": bob})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{block}");
    assert_eq!(block["kind"], "block");
    assert_eq!(
        h.count(
            "SELECT count(*) FROM profile_follows WHERE state = 'ended' AND end_reason = 'blocked'"
        )
        .await,
        1,
        "blocking detaches the follow"
    );
    let (status, _) = h
        .call(
            Method::POST,
            &uri(&alice.profile, "follows"),
            Some(&alice.token),
            Some(json!({"target": bob})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a block rejects new follows"
    );

    let block_id = block["id"].as_str().unwrap();
    let (status, _) = h
        .call(
            Method::DELETE,
            &uri(&alice.profile, &format!("blocks/{block_id}")),
            Some(&alice.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = h
        .call(
            Method::DELETE,
            &uri(&alice.profile, &format!("blocks/{block_id}")),
            Some(&alice.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = h
        .call(
            Method::DELETE,
            &uri(
                &alice.profile,
                &format!("mutes/{}", mute["id"].as_str().unwrap()),
            ),
            Some(&alice.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(h.count("SELECT count(*) FROM profile_blocks").await, 0);
    assert_eq!(h.count("SELECT count(*) FROM profile_mutes").await, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn the_delivery_worker_loop_delivers_and_stops_on_shutdown() {
    let h = Harness::new().await;
    let alice = h.user().await;
    let bob = h.mount_actor("/users/bob").await;
    Mock::given(method("POST"))
        .and(path("/inbox"))
        .respond_with(ResponseTemplate::new(202))
        .mount(&h.peer)
        .await;
    {
        let deps = h.module.clone();
        let profile_id = alice.profile.id().clone();
        let nanoid = alice.profile.nanoid().clone();
        h.module
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
                    .map(|_| ())
                })
            })
            .await
            .unwrap();
    }
    let (status, _) = h
        .call(
            Method::POST,
            &uri(&alice.profile, "follows"),
            Some(&alice.token),
            Some(json!({"target": bob})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let (shutdown, handle) = spawn_profile_delivery_worker(
        Arc::new(h.module.clone()),
        ProfileDeliveryPolicy::default(),
        Duration::from_millis(50),
    );
    let mut delivered = false;
    for _ in 0..200 {
        if h.count("SELECT count(*) FROM profile_delivery_jobs WHERE state = 'delivered'")
            .await
            == 1
        {
            delivered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    shutdown.trigger();
    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("the worker stops after shutdown")
        .unwrap();

    assert!(delivered, "the loop delivered the Follow");
    let inbox_hits = h
        .peer
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "POST")
        .count();
    assert_eq!(inbox_hits, 1);
    let _ = (&alice.account, &h.password);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn following_a_local_profile_completes_through_the_routes_without_a_delivery_job() {
    let h = Harness::new().await;
    let alice = h.user().await;
    let bob = h.user().await;
    let alice_url = format!(
        "{}/ap/profiles/{}",
        h.module.public_base_url().as_str().trim_end_matches('/'),
        alice.profile.nanoid().as_ref()
    );

    let (status, created) = h
        .call(
            Method::POST,
            &uri(&bob.profile, "follows"),
            Some(&bob.token),
            Some(json!({"target": alice_url})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["state"], "accepted");
    assert_eq!(created["target"], alice_url);
    assert_eq!(
        h.count("SELECT count(*) FROM profile_delivery_jobs").await,
        0
    );

    let (status, following) = h
        .call(
            Method::GET,
            &uri(&bob.profile, "following"),
            Some(&bob.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(following["items"].as_array().unwrap().len(), 1);
    let (status, followers) = h
        .call(
            Method::GET,
            &uri(&alice.profile, "followers"),
            Some(&alice.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(followers["items"].as_array().unwrap().len(), 1);

    let follow_id = created["id"].as_str().unwrap();
    let (status, _) = h
        .call(
            Method::DELETE,
            &uri(&bob.profile, &format!("follows/{follow_id}")),
            Some(&bob.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        h.count("SELECT count(*) FROM profile_delivery_jobs").await,
        0
    );
    let _ = (&alice.account, &bob.account);
}
