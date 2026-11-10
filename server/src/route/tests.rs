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
    // Every operation of every unit that implements a Profile route without mounting it
    // (E8 contract, D8P domain-controls, P1 actor, P3i inbox / followers / following) must be
    // among the probes, so dropping an OpenAPI tag cannot silently shrink this check. A unit
    // that mounts a route (P3o / P4) removes it here and adds a presence test instead.
    for (method, template) in [
        ("GET", "/api/v1/profiles/{profile_nanoid}"),
        ("POST", "/api/v1/profiles/{profile_nanoid}/follows"),
        (
            "DELETE",
            "/api/v1/profiles/{profile_nanoid}/follows/{follow_id}",
        ),
        ("GET", "/api/v1/profiles/{profile_nanoid}/following"),
        ("GET", "/api/v1/profiles/{profile_nanoid}/followers"),
        ("GET", "/api/v1/profiles/{profile_nanoid}/blocks"),
        ("POST", "/api/v1/profiles/{profile_nanoid}/mutes"),
        ("GET", "/api/v1/profiles/{profile_nanoid}/domain-controls"),
        (
            "GET",
            "/api/v1/profiles/{profile_nanoid}/domain-controls/{domain}",
        ),
        (
            "PUT",
            "/api/v1/profiles/{profile_nanoid}/domain-controls/{domain}",
        ),
        (
            "DELETE",
            "/api/v1/profiles/{profile_nanoid}/domain-controls/{domain}",
        ),
        ("GET", "/ap/profiles/{profile_nanoid}"),
        ("POST", "/ap/profiles/{profile_nanoid}/inbox"),
    ] {
        assert!(
            probes.contains(&(method.to_string(), replace_path_params(template))),
            "{method} {template} must be probed for absence"
        );
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

/// The same absence check against a world where the routes would visibly work: a real Profile
/// (username, key, owner with a valid token) on a database that has the P2 graph tables. If one of
/// these routes were mounted by mistake it would answer 200 / 401 / 422 instead of 404.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn profile_routes_stay_absent_from_the_root_router_for_a_real_profile_and_owner() {
    use crate::route::profile_graph_test_support::GraphWorld;

    let world = GraphWorld::new().await;
    let owner = world.new_user().await;
    let profile = world.create_profile(&owner, true).await;
    let router = world.root_router();
    let nanoid = &profile.nanoid;

    // Positive controls: the very same requests ARE served when the routes are mounted (test-only
    // routers), so a 404 below really means "not mounted".
    let served = world
        .follow_list_router()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/profiles/{nanoid}/followers"))
                .header("authorization", format!("Bearer {}", owner.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(served.status(), StatusCode::OK);
    let served = world
        .inbox_router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/ap/profiles/{nanoid}/inbox"))
                .header("content-type", "application/activity+json")
                .header("host", world.host_header.clone())
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(served.status(), StatusCode::UNAUTHORIZED);

    let mut probes: Vec<(&str, String)> = Vec::new();
    for suffix in [
        "",
        "/follows",
        "/following",
        "/followers",
        "/blocks",
        "/mutes",
        "/domain-controls",
        "/domain-controls/example.com",
    ] {
        for method in ["GET", "POST", "PUT", "DELETE"] {
            probes.push((method, format!("/api/v1/profiles/{nanoid}{suffix}")));
        }
    }
    for suffix in ["", "/inbox", "/outbox", "/followers", "/following"] {
        for method in ["GET", "POST"] {
            probes.push((method, format!("/ap/profiles/{nanoid}{suffix}")));
        }
    }
    for (method, uri) in &probes {
        for token in [Some(owner.token.as_str()), None] {
            let mut builder = Request::builder()
                .method(*method)
                .uri(uri)
                .header("content-type", "application/activity+json");
            if let Some(token) = token {
                builder = builder.header("authorization", format!("Bearer {token}"));
            }
            let response = router
                .clone()
                .oneshot(builder.body(Body::from("{}")).unwrap())
                .await
                .unwrap();
            let status = response.status();
            // The authentication layer of the /api/v1 subtree answers before routing.
            if status == StatusCode::UNAUTHORIZED && token.is_none() && uri.starts_with("/api/v1") {
                continue;
            }
            assert!(
                status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED,
                "{method} {uri} (authenticated: {}) must not be served, got {status}",
                token.is_some()
            );
        }
    }
}

/// Production serves exactly what `build_root_router` builds (the router the absence test above
/// probes): `main` must not add routes of its own on top of it.
#[test]
fn main_serves_only_the_root_router() {
    let main_rs = include_str!("../main.rs");
    assert!(main_rs.contains("root_router::build_root_router("));
    for forbidden in [".route(", ".nest(", ".merge(", "Router::new"] {
        assert!(
            !main_rs.contains(forbidden),
            "main.rs must not build routes itself ({forbidden})"
        );
    }
}

/// The production entrypoint starts projection, Registration Gate webhook, and opt-in registration
/// maintenance. A new (Profile relationship) delivery worker must not be started from `main`
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
        3,
        "unexpected worker start in main.rs: {spawns:?}"
    );
    assert!(spawns[0].contains("ProjectionWorker::spawn"), "{spawns:?}");
    assert!(
        spawns[1].contains("webhook_worker::spawn_if_enabled"),
        "{spawns:?}"
    );
    assert!(
        spawns[2].contains("registration_guard::spawn_if_enabled"),
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
