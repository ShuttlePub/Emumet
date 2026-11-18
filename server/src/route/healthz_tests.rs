use super::{HealthzRouter, HEALTHZ_BODY};
use crate::handler::AppModule;
use crate::route::build_test_router;
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn call(router: axum::Router, method: Method, uri: &str) -> (StatusCode, String, String) {
    let response = router
        .oneshot(
            Request::builder()
                .method(method)
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
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        content_type,
        String::from_utf8(body.to_vec()).unwrap(),
    )
}

/// The route needs no state at all: this runs without any database or external service.
#[tokio::test]
async fn healthz_answers_ok_with_a_fixed_body_without_any_state() {
    let router = axum::Router::<()>::new().route_healthz();
    let (status, content_type, body) = call(router.clone(), Method::GET, "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_bytes(), b"ok\n");
    assert_eq!(body, HEALTHZ_BODY);
    assert_eq!(content_type, "text/plain; charset=utf-8");

    // Only GET (and the HEAD axum derives from it) is routed.
    let (status, _, _) = call(router.clone(), Method::POST, "/healthz").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    let (status, _, _) = call(router, Method::GET, "/healthz/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// On the production root router `/healthz` is public (no bearer token), answers the same bytes,
/// is not in the OpenAPI document and does not shadow any other route.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn healthz_is_public_on_the_root_router_and_absent_from_openapi() {
    let module = AppModule::new_for_oauth2_test(
        "http://localhost:65535".into(),
        "http://localhost:65535".into(),
    )
    .await
    .expect("AppModule init failed (is DATABASE_URL set?)");
    let router = build_test_router(module);

    let (status, _, body) = call(router.clone(), Method::GET, "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok\n");

    // Neighbouring authenticated routes still demand a token.
    let (status, _, _) = call(router, Method::GET, "/api/v1/me").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    assert!(!crate::openapi::generate_openapi_json().contains("healthz"));
}
