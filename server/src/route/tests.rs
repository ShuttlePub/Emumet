use super::build_test_router;
use crate::handler::AppModule;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

const HTTP_METHODS: &[&str] = &[
    "get", "post", "put", "delete", "patch", "options", "head", "trace",
];

async fn app() -> axum::Router {
    let module = AppModule::new_for_oauth2_test(
        "http://localhost:65535".into(),
        "http://localhost:65535".into(),
    )
    .await
    .expect("AppModule init failed (is DATABASE_URL set?)");
    build_test_router(module)
}

struct RouteCase {
    method: String,
    uri: String,
    requires_auth: bool,
    allows_not_found: bool,
    accept: Option<&'static str>,
}

fn replace_path_params(template: &str) -> String {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars();
    while let Some(c) = chars.next() {
        if c == '{' {
            let param_name: String = chars.by_ref().take_while(|&ch| ch != '}').collect();
            result.push_str(&format!("test-{param_name}"));
        } else {
            result.push(c);
        }
    }
    result
}

fn is_authed(security: Option<&serde_json::Value>) -> bool {
    match security {
        Some(arr) => arr.as_array().is_some_and(|a| !a.is_empty()),
        None => false,
    }
}

fn has_not_found_response(operation: &serde_json::Value) -> bool {
    operation
        .get("responses")
        .and_then(|responses| responses.as_object())
        .is_some_and(|responses| responses.contains_key("404"))
}

/// `ProfilePolicy` operations are OpenAPI contract only and are intentionally not mounted on the
/// router under test (see `route::profile_policy`); their reachability is covered separately.
fn is_contract_only_operation(operation: &serde_json::Value) -> bool {
    operation
        .get("tags")
        .and_then(|tags| tags.as_array())
        .into_iter()
        .flatten()
        .any(|tag| matches!(tag.as_str(), Some("ProfilePolicy" | "ProfileActor")))
}

fn is_activitypub_operation(operation: &serde_json::Value) -> bool {
    operation
        .get("tags")
        .and_then(|tags| tags.as_array())
        .into_iter()
        .flatten()
        .any(|tag| tag.as_str() == Some("ActivityPub"))
}

fn load_routes_from_openapi() -> Vec<RouteCase> {
    let spec_json = crate::openapi::generate_openapi_json();
    let spec: serde_json::Value =
        serde_json::from_str(&spec_json).expect("Failed to parse generated OpenAPI spec");
    let paths = spec["paths"].as_object().expect("paths must be an object");
    let mut cases = Vec::new();

    for (path_template, path_item) in paths {
        let path_item = path_item.as_object().expect("path item must be an object");
        for (key, operation) in path_item {
            if !HTTP_METHODS.contains(&key.as_str()) {
                continue;
            }
            if is_contract_only_operation(operation) {
                continue;
            }
            let requires_auth = is_authed(operation.get("security"));
            let concrete_path = replace_path_params(path_template);
            let query_params: Vec<String> = operation
                .get("parameters")
                .and_then(|p| p.as_array())
                .into_iter()
                .flatten()
                .filter(|p| p["in"] == "query" && p["required"] == true)
                .map(|p| format!("{}=smoke", p["name"].as_str().unwrap_or("unknown")))
                .collect();
            let uri = if query_params.is_empty() {
                concrete_path
            } else {
                format!("{}?{}", concrete_path, query_params.join("&"))
            };
            cases.push(RouteCase {
                method: key.to_uppercase(),
                uri,
                requires_auth,
                allows_not_found: !requires_auth && has_not_found_response(operation),
                accept: is_activitypub_operation(operation).then_some("application/activity+json"),
            });
        }
    }
    cases
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn all_openapi_routes_are_reachable() {
    let cases = load_routes_from_openapi();
    assert!(!cases.is_empty(), "No routes found in OpenAPI spec");
    let router = app().await;

    for case in &cases {
        let mut request = Request::builder()
            .method(case.method.as_str())
            .uri(&case.uri)
            .header("content-type", "application/json");
        if let Some(accept) = case.accept {
            request = request.header("accept", accept);
        }
        let request = request.body(Body::from("{}")).unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();

        if !case.allows_not_found {
            assert_ne!(
                status,
                StatusCode::NOT_FOUND,
                "Route {} {} returned 404 — is it registered?",
                case.method,
                case.uri,
            );
        }
        assert_ne!(
            status,
            StatusCode::METHOD_NOT_ALLOWED,
            "Route {} {} returned 405 — method mismatch?",
            case.method,
            case.uri,
        );
        if case.requires_auth {
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "Authed route {} {} expected 401 without token, got {}",
                case.method,
                case.uri,
                status,
            );
        } else {
            assert!(
                status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
                "Public route {} {} returned {} — accidentally wrapped with auth middleware?",
                case.method,
                case.uri,
                status,
            );
            assert!(
                !status.is_server_error() || status == StatusCode::BAD_GATEWAY,
                "Public route {} {} returned {} — handler or wiring broken?",
                case.method,
                case.uri,
                status,
            );
        }
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn nonexistent_route_returns_404() {
    let router = app().await;
    let request = Request::builder()
        .method("GET")
        .uri("/this/route/does/not/exist")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// The production root router (built by the same function the server binary uses) must not serve
/// any Profile actor / relationship / domain-control route before the P4 cutover. Every operation
/// of the not-yet-mounted OpenAPI tags, plus the actor sub-resources, must be a plain 404 even for
/// an authenticated caller, and the Account based actor routes must still be there.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn profile_actor_and_contract_routes_are_absent_from_the_root_router() {
    use crate::auth::{
        encode_test_jwt, generate_test_keys, AuthClaims, JwksCache, OidcConfig, OneOrMany,
    };
    use crate::route::build_test_router_with_auth;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    let issuer = format!("https://issuer.example/{}", uuid::Uuid::new_v4());
    let keys = generate_test_keys();
    let token = encode_test_jwt(
        &AuthClaims {
            iss: issuer.clone(),
            sub: "absence-subject".to_string(),
            aud: OneOrMany::One("emumet".to_string()),
            exp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 3600,
        },
        &keys.encoding_key,
        &keys.kid,
    );
    let module = AppModule::new_for_oauth2_test(
        "http://localhost:65535".into(),
        "http://localhost:65535".into(),
    )
    .await
    .expect("AppModule init failed (is DATABASE_URL set?)");
    let router = build_test_router_with_auth(
        module,
        Arc::new(OidcConfig {
            issuer_url: issuer.clone(),
            expected_audience: "emumet".to_string(),
            jwks_refetch_interval_secs: 0,
        }),
        Arc::new(JwksCache::new_with_jwks(issuer, keys.jwk_set)),
    );

    let spec: serde_json::Value =
        serde_json::from_str(&crate::openapi::generate_openapi_json()).unwrap();
    let mut probes: Vec<(String, String)> = Vec::new();
    for (template, item) in spec["paths"].as_object().unwrap() {
        for (method, operation) in item.as_object().unwrap() {
            let unmounted = operation["tags"].as_array().is_some_and(|tags| {
                tags.iter()
                    .any(|tag| tag == "ProfilePolicy" || tag == "ProfileActor")
            });
            if unmounted && HTTP_METHODS.contains(&method.as_str()) {
                probes.push((method.to_uppercase(), replace_path_params(template)));
            }
        }
    }
    assert!(
        probes.len() >= 16,
        "expected the ProfilePolicy contract and the Profile actor, found {} operations",
        probes.len()
    );
    for suffix in ["/inbox", "/outbox", "/followers", "/following"] {
        for method in ["GET", "POST"] {
            probes.push((
                method.to_string(),
                format!("/ap/profiles/test-nanoid{suffix}"),
            ));
        }
    }
    for path in ["/ap/profiles", "/ap/profiles/test-nanoid"] {
        probes.push(("POST".to_string(), path.to_string()));
    }

    for (method, uri) in &probes {
        for authenticated in [true, false] {
            let mut builder = Request::builder()
                .method(method.as_str())
                .uri(uri)
                .header("content-type", "application/json");
            if authenticated {
                builder = builder.header("authorization", format!("Bearer {token}"));
            }
            let response = router
                .clone()
                .oneshot(builder.body(Body::from("{}")).unwrap())
                .await
                .unwrap();
            let status = response.status();
            assert!(
                status == StatusCode::NOT_FOUND
                    || (!authenticated && status == StatusCode::UNAUTHORIZED),
                "{method} {uri} (authenticated: {authenticated}) must not be served by the root router, got {status}"
            );
            if authenticated {
                assert_eq!(
                    status,
                    StatusCode::NOT_FOUND,
                    "{method} {uri} must be absent for an authenticated caller too"
                );
            }
        }
    }

    // Positive control: the Account based federation routes of the same router are served.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/ap/accounts/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "unknown Account actor"
    );
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/profiles/test-nanoid/username")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "the one-time username setter is mounted (and authenticated)"
    );
}

/// The production entrypoint starts exactly the projection worker and the Registration Gate
/// webhook worker. A new (Profile relationship) delivery worker must not be started from `main`
/// before P4 owns the cutover; whoever adds one has to update this list deliberately.
#[test]
fn main_starts_no_profile_delivery_worker() {
    let main_rs = include_str!("../main.rs");
    let spawns: Vec<&str> = main_rs
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//") && line.contains("spawn"))
        .collect();
    assert_eq!(
        spawns.len(),
        2,
        "unexpected worker start in main.rs: {spawns:?}"
    );
    assert!(spawns[0].contains("ProjectionWorker::spawn"), "{spawns:?}");
    assert!(
        spawns[1].contains("webhook_worker::spawn_if_enabled"),
        "{spawns:?}"
    );
    for forbidden in [
        "tokio::spawn",
        "profile_delivery",
        "ProfileDelivery",
        "profile_relationship",
    ] {
        assert!(
            !main_rs.contains(forbidden),
            "main.rs must not contain {forbidden}"
        );
    }
}
