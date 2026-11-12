//! Profile actor document and WebFinger over HTTP (real database). The handlers are mounted on a
//! test-only router through `ProfileActorRouter`, exactly the way P4 will mount them.

use super::ProfileActorRouter;
use crate::handler::AppModule;
use application::dto::account::CreateAccountDto;
use application::profile_signing_key::CreateProfileSigningKeyUseCase;
use application::service::account::CreateAccountUseCase;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::crypto::SigningAlgorithm;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnDatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::prelude::entity::{AuthAccountId, Nanoid, Profile, ProfileId};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

struct World {
    module: AppModule,
    router: axum::Router,
    base_url: String,
    host: String,
    _keto: MockServer,
}

async fn world() -> World {
    ensure_master_key_password_file();
    kernel::ensure_generator_initialized();
    let keto = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/admin/relation-tuples"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&keto)
        .await;
    let module = AppModule::new_for_test_urls(
        "http://localhost:65535".to_string(),
        "http://localhost:65535".to_string(),
        keto.uri(),
        keto.uri(),
    )
    .await
    .expect("AppModule init (is DATABASE_URL set?)");
    let base_url = module
        .public_base_url()
        .as_str()
        .trim_end_matches('/')
        .to_string();
    let host = url::Url::parse(&base_url)
        .unwrap()
        .host_str()
        .unwrap()
        .to_string();
    let host = match url::Url::parse(&base_url).unwrap().port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    let router = axum::Router::new()
        .nest("/ap", axum::Router::new().route_profile_actor())
        .route_profile_webfinger()
        .with_state(module.clone());
    World {
        module,
        router,
        base_url,
        host,
        _keto: keto,
    }
}

struct Fixture {
    profile_id: ProfileId,
    nanoid: String,
}

async fn create_profile(world: &World, username: Option<&str>) -> Fixture {
    let auth_account_id = AuthAccountId::default();
    let mut conn = world
        .module
        .database_connection()
        .connection()
        .await
        .unwrap();
    let host_id = kernel::generate_id();
    sqlx::query("INSERT INTO auth_hosts (id, url) VALUES ($1, $2)")
        .bind(host_id)
        .bind(format!("https://auth-{host_id}.example.com"))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO auth_accounts (id, host_id, client_id) VALUES ($1, $2, $3)")
        .bind(auth_account_id.as_ref())
        .bind(host_id)
        .bind(format!("client-{host_id}"))
        .execute(&mut *conn)
        .await
        .unwrap();
    let name = kernel::test_utils::unique_account_name()
        .as_ref()
        .to_string();
    world
        .module
        .create_account_with_profile_username(
            auth_account_id,
            CreateAccountDto {
                name: name.clone(),
                is_bot: false,
            },
            username
                .map(str::to_string)
                .unwrap_or_else(|| format!("u{}", uuid::Uuid::new_v4().simple())),
        )
        .await
        .unwrap();
    let (id, nanoid): (i64, String) = sqlx::query_as(
        "SELECT p.id, p.nanoid FROM profiles p JOIN accounts a ON a.id = p.account_id WHERE a.name = $1",
    )
    .bind(&name)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    // Creation gives every Profile its key (and a username); these tests start from the stripped
    // state and add the key (and username) they need.
    sqlx::query("DELETE FROM profile_signing_keys WHERE profile_id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await
        .unwrap();
    if username.is_none() {
        sqlx::query("UPDATE profiles SET username = NULL WHERE id = $1")
            .bind(id)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    Fixture {
        profile_id: ProfileId::new(id),
        nanoid,
    }
}

async fn create_key(world: &World, fixture: &Fixture) {
    let deps = world.module.clone();
    let profile_id = fixture.profile_id.clone();
    let nanoid = Nanoid::<Profile>::new(fixture.nanoid.clone());
    world
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
}

async fn get(world: &World, uri: &str) -> (StatusCode, Option<String>, serde_json::Value) {
    let response = world
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, content_type, json)
}

fn unique_username() -> String {
    format!("u{}", uuid::Uuid::new_v4().simple())
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn actor_document_is_served_only_for_profiles_with_username_and_key() {
    let world = world().await;
    let username = unique_username();
    let named = create_profile(&world, Some(&username)).await;

    // Username but no key yet, and unknown Profiles: 404.
    assert_eq!(
        get(&world, &format!("/ap/profiles/{}", named.nanoid))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&world, "/ap/profiles/no-such-profile").await.0,
        StatusCode::NOT_FOUND
    );
    // Key but no username: still 404.
    let keyed_only = create_profile(&world, None).await;
    create_key(&world, &keyed_only).await;
    assert_eq!(
        get(&world, &format!("/ap/profiles/{}", keyed_only.nanoid))
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    create_key(&world, &named).await;
    let (status, content_type, json) = get(&world, &format!("/ap/profiles/{}", named.nanoid)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("application/activity+json"));
    let actor_id = format!("{}/ap/profiles/{}", world.base_url, named.nanoid);
    assert_eq!(json["id"], actor_id.as_str());
    assert_eq!(json["type"], "Person");
    assert_eq!(json["preferredUsername"], username.as_str());
    assert_eq!(json["url"], actor_id.as_str());
    assert_eq!(json["inbox"], format!("{actor_id}/inbox"));
    assert_eq!(json["outbox"], format!("{actor_id}/outbox"));
    assert_eq!(json["followers"], format!("{actor_id}/followers"));
    assert_eq!(json["following"], format!("{actor_id}/following"));
    assert_eq!(json["publicKey"]["id"], format!("{actor_id}#main-key"));
    assert_eq!(json["publicKey"]["owner"], actor_id.as_str());
    assert!(json["publicKey"]["publicKeyPem"]
        .as_str()
        .unwrap()
        .starts_with("-----BEGIN PUBLIC KEY-----"));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn webfinger_resolves_profile_acct_to_the_profile_actor() {
    let world = world().await;
    let username = unique_username();
    let fixture = create_profile(&world, Some(&username)).await;
    let resource =
        |name: &str, host: &str| format!("/.well-known/webfinger?resource=acct:{name}@{host}");

    // Not an actor before it has a key.
    assert_eq!(
        get(&world, &resource(&username, &world.host)).await.0,
        StatusCode::NOT_FOUND
    );
    create_key(&world, &fixture).await;

    let (status, content_type, json) = get(&world, &resource(&username, &world.host)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("application/jrd+json"));
    let actor_id = format!("{}/ap/profiles/{}", world.base_url, fixture.nanoid);
    assert_eq!(
        json["subject"],
        format!("acct:{username}@{}", world.host).as_str()
    );
    assert_eq!(json["links"][0]["rel"], "self");
    assert_eq!(json["links"][0]["type"], "application/activity+json");
    assert_eq!(json["links"][0]["href"], actor_id.as_str());
    assert_eq!(json["aliases"][0], actor_id.as_str());

    // The domain must be this instance, the name must exist, the resource must be well-formed.
    assert_eq!(
        get(&world, &resource(&username, "other.example")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&world, &resource("no-such-profile", &world.host))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&world, "/.well-known/webfinger?resource=nonsense")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&world, "/.well-known/webfinger").await.0,
        StatusCode::BAD_REQUEST
    );
}
