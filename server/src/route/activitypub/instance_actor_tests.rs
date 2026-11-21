//! The instance actor document over HTTP on the production root router (real database).

use crate::handler::AppModule;
use crate::route::build_test_router;
use application::instance_identity::EnsureInstanceIdentityUseCase;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use driver::crypto::FilePasswordProvider;
use driver::database::isolated_db::IsolatedDatabase;
use http_body_util::BodyExt;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::prelude::entity::InstanceOrigin;
use tower::ServiceExt;

struct World {
    _iso: IsolatedDatabase,
    _password: tempfile::NamedTempFile,
    module: AppModule,
    router: axum::Router,
    origin: InstanceOrigin,
}

async fn world() -> World {
    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create().await;
    let password = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(password.path(), b"instance-actor-route-password").unwrap();
    let module = AppModule::new_for_oauth2_test(
        "http://localhost:65535".to_string(),
        "http://localhost:65535".to_string(),
    )
    .await
    .expect("AppModule init failed (is DATABASE_URL set?)")
    .with_database(iso.database.clone())
    .with_password_provider(FilePasswordProvider::with_paths(
        password.path(),
        password.path(),
    ));
    let origin = InstanceOrigin::parse(module.public_base_url().as_str()).unwrap();
    let router = build_test_router(module.clone());
    World {
        _iso: iso,
        _password: password,
        module,
        router,
        origin,
    }
}

async fn get(
    router: &axum::Router,
    path: &str,
    host: Option<&str>,
) -> (StatusCode, String, String) {
    let mut request = Request::builder().method("GET").uri(path);
    if let Some(host) = host {
        request = request.header(header::HOST, host);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        content_type,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn instance_actor_is_served_anonymously_as_an_application() {
    let world = world().await;
    let report = world.module.ensure_instance_identity().await.unwrap();

    let (status, content_type, body) = get(&world.router, "/ap/instance", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("application/activity+json"));
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["id"], world.origin.actor_id());
    assert_eq!(json["type"], "Application");
    let keys = json["publicKey"].as_array().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["id"], report.key_id_uri);
    assert_eq!(keys[0]["owner"], world.origin.actor_id());
    assert!(keys[0]["publicKeyPem"]
        .as_str()
        .unwrap()
        .starts_with("-----BEGIN PUBLIC KEY-----"));
    assert!(!body.contains("PRIVATE"));
    assert!(!body.contains("ciphertext"));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn instance_actor_urls_come_from_the_configuration_not_the_request_host() {
    let world = world().await;
    world.module.ensure_instance_identity().await.unwrap();
    let (_, _, plain) = get(&world.router, "/ap/instance", None).await;
    let (status, _, spoofed) = get(&world.router, "/ap/instance", Some("evil.example")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(spoofed, plain);
    assert!(!spoofed.contains("evil.example"));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn instance_actor_is_a_server_error_until_the_identity_is_provisioned() {
    let world = world().await;
    let (status, _, _) = get(&world.router, "/ap/instance", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn profile_actor_routes_are_unaffected_by_the_instance_actor() {
    let world = world().await;
    world.module.ensure_instance_identity().await.unwrap();
    let (status, _, _) = get(&world.router, "/ap/profiles/no-such-profile", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Account based federation paths stay retired.
    let (status, _, _) = get(&world.router, "/ap/accounts/1", None).await;
    assert_eq!(status, StatusCode::GONE);
}
