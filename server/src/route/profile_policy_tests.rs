use super::ProfilePolicyContractRouter;
use crate::auth::{
    auth_middleware, encode_test_jwt, generate_test_keys, AuthClaims, JwksCache, OidcConfig,
};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

const AUDIENCE: &str = "emumet";
const HTTP_METHODS: &[&str] = &["get", "post", "put", "delete", "patch"];

struct ContractApp {
    router: axum::Router,
    token: String,
}

fn contract_app() -> ContractApp {
    let issuer = format!("https://issuer.example/{}", uuid::Uuid::new_v4());
    let keys = generate_test_keys();
    let claims = AuthClaims {
        iss: issuer.clone(),
        sub: "contract-subject".to_string(),
        aud: crate::auth::OneOrMany::One(AUDIENCE.to_string()),
        exp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_secs()
            + 3600,
    };
    let token = encode_test_jwt(&claims, &keys.encoding_key, &keys.kid);
    let oidc_config = Arc::new(OidcConfig {
        issuer_url: issuer.clone(),
        expected_audience: AUDIENCE.to_string(),
        jwks_refetch_interval_secs: 0,
    });
    let jwks_cache = Arc::new(JwksCache::new_with_jwks(issuer, keys.jwk_set));
    let authed = axum::Router::new()
        .nest(
            "/api/v1",
            axum::Router::new().route_profile_policy_contract(),
        )
        .layer(axum::middleware::from_fn_with_state(
            (oidc_config, jwks_cache),
            auth_middleware,
        ));
    let public =
        axum::Router::new().nest("/ap", axum::Router::new().route_profile_actor_contract());
    ContractApp {
        router: authed.merge(public),
        token,
    }
}

struct ContractOperation {
    method: String,
    uri: String,
    requires_auth: bool,
}

fn contract_operations() -> Vec<ContractOperation> {
    let spec: serde_json::Value = serde_json::from_str(&crate::openapi::generate_openapi_json())
        .expect("generated OpenAPI spec is valid JSON");
    let mut operations = Vec::new();
    for (template, item) in spec["paths"].as_object().unwrap() {
        for (method, operation) in item.as_object().unwrap() {
            if !HTTP_METHODS.contains(&method.as_str()) {
                continue;
            }
            let is_contract = operation["tags"]
                .as_array()
                .is_some_and(|tags| tags.iter().any(|t| t == "ProfilePolicy"));
            if !is_contract {
                continue;
            }
            let mut uri = String::new();
            let mut chars = template.chars();
            while let Some(c) = chars.next() {
                if c == '{' {
                    let name: String = chars.by_ref().take_while(|&ch| ch != '}').collect();
                    uri.push_str(&format!("sample-{name}"));
                } else {
                    uri.push(c);
                }
            }
            let required_query: Vec<String> = operation["parameters"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| p["in"] == "query" && p["required"] == true)
                .map(|p| format!("{}=1", p["name"].as_str().unwrap()))
                .collect();
            if !required_query.is_empty() {
                uri = format!("{uri}?{}", required_query.join("&"));
            }
            operations.push(ContractOperation {
                method: method.to_uppercase(),
                uri,
                requires_auth: operation["security"]
                    .as_array()
                    .is_some_and(|s| !s.is_empty()),
            });
        }
    }
    operations
}

fn build_request(operation: &ContractOperation, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(operation.method.as_str())
        .uri(&operation.uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from("{}")).unwrap()
}

#[tokio::test]
async fn every_contract_operation_answers_explicit_501_when_authenticated() {
    let app = contract_app();
    let operations = contract_operations();
    assert_eq!(
        operations.len(),
        16,
        "expected the full ProfilePolicy contract, found {} operations",
        operations.len()
    );
    for operation in &operations {
        let response = app
            .router
            .clone()
            .oneshot(build_request(operation, Some(&app.token)))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_IMPLEMENTED,
            "{} {} must be an explicit 501 skeleton",
            operation.method,
            operation.uri
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("not implemented"),
            "{} {} 501 body must say so, got {text:?}",
            operation.method,
            operation.uri
        );
    }
}

#[tokio::test]
async fn authenticated_contract_operations_reject_missing_token() {
    let app = contract_app();
    for operation in contract_operations()
        .iter()
        .filter(|operation| operation.requires_auth)
    {
        let response = app
            .router
            .clone()
            .oneshot(build_request(operation, None))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{} {} must require a bearer token",
            operation.method,
            operation.uri
        );
    }
}

#[test]
fn production_entrypoint_does_not_mount_contract_routes() {
    let main_rs = include_str!("../main.rs");
    for needle in [
        "profile_policy",
        "ProfilePolicyContractRouter",
        "route_profile_policy_contract",
        "route_profile_actor_contract",
    ] {
        assert!(
            !main_rs.contains(needle),
            "main.rs must not reference {needle}: contract routes are mounted in P4 at the earliest"
        );
    }
}
