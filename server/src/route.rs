use crate::error::ErrorStatus;
use application::transfer::pagination::Direction;
use axum::http::StatusCode;

pub mod account;
pub mod metadata;
pub mod oauth2;
pub mod profile;

const MAX_BATCH_SIZE: usize = 100;

fn parse_comma_ids(raw: &str) -> Result<Vec<String>, ErrorStatus> {
    let ids: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "ID list cannot be empty".to_string(),
        )));
    }
    if ids.len() > MAX_BATCH_SIZE {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            format!("Too many IDs: maximum is {MAX_BATCH_SIZE}"),
        )));
    }
    Ok(ids)
}

trait DirectionConverter {
    fn convert_to_direction(self) -> Result<Direction, ErrorStatus>;
}

impl DirectionConverter for Option<String> {
    fn convert_to_direction(self) -> Result<Direction, ErrorStatus> {
        match self {
            Some(d) => match Direction::try_from(d) {
                Ok(d) => Ok(d),
                Err(message) => Err((StatusCode::BAD_REQUEST, message).into()),
            },
            None => Ok(Direction::default()),
        }
    }
}

#[cfg(test)]
pub(crate) fn build_test_router(app: crate::handler::AppModule) -> axum::Router {
    use crate::auth::{JwksCache, OidcConfig};
    use crate::route::account::AccountRouter;
    use crate::route::metadata::MetadataRouter;
    use crate::route::oauth2::OAuth2Router;
    use crate::route::profile::ProfileRouter;
    use std::sync::Arc;
    use std::time::Duration;

    let oidc_config = Arc::new(OidcConfig {
        issuer_url: "http://localhost:4444".to_string(),
        expected_audience: "emumet".to_string(),
        jwks_refetch_interval_secs: 0,
    });
    let jwks_cache = Arc::new(JwksCache::new(
        oidc_config.issuer_url.clone(),
        Duration::from_secs(0),
    ));

    let authed_routes = axum::Router::new()
        .route_account()
        .route_profile()
        .route_metadata()
        .layer(axum::middleware::from_fn_with_state(
            (oidc_config, jwks_cache),
            crate::auth::auth_middleware,
        ));

    let public_routes = axum::Router::new().route_oauth2();

    authed_routes.merge(public_routes).with_state(app)
}

#[cfg(test)]
mod route_smoke_tests {
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
            let request = Request::builder()
                .method(case.method.as_str())
                .uri(&case.uri)
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap();

            let response = router.clone().oneshot(request).await.unwrap();
            let status = response.status();

            assert_ne!(
                status,
                StatusCode::NOT_FOUND,
                "Route {} {} returned 404 — is it registered?",
                case.method,
                case.uri,
            );
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
}
