use crate::auth::{resolve_auth_account_id, AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::schema::me::MeResponse;
use application::service::session_context::GetSessionContextUseCase;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use kernel::interfaces::permission::InstanceRole;

pub trait MeRouter {
    fn route_me(self) -> Self;
}

impl MeRouter for axum::Router<AppModule> {
    fn route_me(self) -> Self {
        self.route("/me", axum::routing::get(get_me))
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/me",
    description = "Retrieve the authenticated account and direct instance roles.",
    responses(
        (status = 200, description = "Authenticated session", body = MeResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 503, description = "Instance role service unavailable"),
    ),
    security(("bearer_auth" = [])),
    tag = "Me",
)]
pub(crate) async fn get_me(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
) -> Result<Json<MeResponse>, ErrorStatus> {
    let auth_account_id = resolve_auth_account_id(&module, OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    let session_context = module
        .get_session_context(&auth_account_id)
        .await
        .map_err(|_| ErrorStatus::StatusCode(StatusCode::SERVICE_UNAVAILABLE))?;
    let instance_roles = session_context
        .instance_roles
        .into_iter()
        .map(|role| match role {
            InstanceRole::Admin => "admin".to_string(),
            InstanceRole::Moderator => "moderator".to_string(),
        })
        .collect();

    Ok(Json(MeResponse {
        account_id: AsRef::<i64>::as_ref(&auth_account_id).to_string(),
        instance_roles,
    }))
}

#[cfg(test)]
mod tests {
    use crate::auth::{encode_test_jwt, generate_test_keys, AuthClaims, JwksCache, OidcConfig};
    use crate::handler::AppModule;
    use crate::route::build_test_router_with_auth;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
    use kernel::interfaces::read_model::{AuthAccountReadModel, DependOnAuthAccountReadModel};
    use kernel::interfaces::repository::{AuthHostRepository, DependOnAuthHostRepository};
    use kernel::prelude::entity::{AuthAccountId, AuthHostId};
    use kernel::test_utils::{AuthAccountBuilder, AuthHostBuilder};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const AUDIENCE: &str = "emumet";

    struct AuthenticatedRouter {
        router: axum::Router,
        token: String,
        auth_account_id: AuthAccountId,
    }

    fn claims(issuer: &str, subject: &str) -> AuthClaims {
        let exp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_secs()
            + 3600;
        AuthClaims {
            iss: issuer.to_string(),
            sub: subject.to_string(),
            aud: crate::auth::OneOrMany::One(AUDIENCE.to_string()),
            exp,
        }
    }

    async fn authenticated_router(keto_read_url: &str) -> AuthenticatedRouter {
        kernel::ensure_generator_initialized();
        let issuer = format!("https://issuer.example/{}", uuid::Uuid::new_v4());
        let subject = format!("subject-{}", uuid::Uuid::new_v4());
        let module = AppModule::new_for_test_urls(
            "http://localhost:65535".to_string(),
            "http://localhost:65535".to_string(),
            keto_read_url.to_string(),
            "http://localhost:65535".to_string(),
        )
        .await
        .expect("AppModule init failed (is DATABASE_URL set?)");

        let host_id = AuthHostId::default();
        let auth_host = AuthHostBuilder::new()
            .id(host_id.clone())
            .url(issuer.clone())
            .build();
        let auth_account_id = AuthAccountId::default();
        let auth_account = AuthAccountBuilder::new()
            .id(auth_account_id.clone())
            .host(host_id)
            .client_id(subject.clone())
            .build();
        let mut executor = module
            .database_connection()
            .connection()
            .await
            .expect("database executor");
        module
            .auth_host_repository()
            .create(&mut executor, &auth_host)
            .await
            .expect("seed auth host");
        module
            .auth_account_read_model()
            .create(&mut executor, &auth_account)
            .await
            .expect("seed auth account");

        let keys = generate_test_keys();
        let token = encode_test_jwt(&claims(&issuer, &subject), &keys.encoding_key, &keys.kid);
        let oidc_config = Arc::new(OidcConfig {
            issuer_url: issuer.clone(),
            expected_audience: AUDIENCE.to_string(),
            jwks_refetch_interval_secs: 0,
        });
        let jwks_cache = Arc::new(JwksCache::new_with_jwks(issuer, keys.jwk_set));

        AuthenticatedRouter {
            router: build_test_router_with_auth(module, oidc_config, jwks_cache),
            token,
            auth_account_id,
        }
    }

    fn me_request(token: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri("/api/v1/me")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .expect("valid request")
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let body = response
            .into_body()
            .collect()
            .await
            .expect("response body")
            .to_bytes();
        serde_json::from_slice(&body).expect("JSON response")
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn get_me_returns_auth_account_and_admin_role_when_keto_returns_admins_tuple() {
        let keto = MockServer::start().await;
        let app = authenticated_router(&keto.uri()).await;
        let subject_id = AsRef::<i64>::as_ref(&app.auth_account_id).to_string();
        Mock::given(method("GET"))
            .and(path("/relation-tuples"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "relation_tuples": [{
                    "namespace": "Instance",
                    "object": "singleton",
                    "relation": "admins",
                    "subject_id": subject_id,
                }],
                "next_page_token": "",
            })))
            .expect(1)
            .mount(&keto)
            .await;

        let response = app.router.oneshot(me_request(&app.token)).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await,
            serde_json::json!({
                "account_id": AsRef::<i64>::as_ref(&app.auth_account_id).to_string(),
                "instance_roles": ["admin"],
            })
        );
        keto.verify().await;
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn get_me_returns_empty_roles_when_keto_returns_no_relation_tuples() {
        let keto = MockServer::start().await;
        let app = authenticated_router(&keto.uri()).await;
        Mock::given(method("GET"))
            .and(path("/relation-tuples"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "relation_tuples": [],
                "next_page_token": "",
            })))
            .expect(1)
            .mount(&keto)
            .await;

        let response = app.router.oneshot(me_request(&app.token)).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await,
            serde_json::json!({
                "account_id": AsRef::<i64>::as_ref(&app.auth_account_id).to_string(),
                "instance_roles": [],
            })
        );
        keto.verify().await;
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn get_me_returns_service_unavailable_when_keto_returns_500() {
        let keto = MockServer::start().await;
        let app = authenticated_router(&keto.uri()).await;
        Mock::given(method("GET"))
            .and(path("/relation-tuples"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&keto)
            .await;

        let response = app.router.oneshot(me_request(&app.token)).await.unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        keto.verify().await;
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn get_me_returns_service_unavailable_when_keto_connection_is_refused() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve local port");
        let port = listener
            .local_addr()
            .expect("reserved local address")
            .port();
        drop(listener);
        let app = authenticated_router(&format!("http://127.0.0.1:{port}")).await;

        let response = app.router.oneshot(me_request(&app.token)).await.unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn get_me_returns_unauthorized_when_authorization_header_is_missing() {
        let keto = MockServer::start().await;
        let app = authenticated_router(&keto.uri()).await;
        let request = Request::builder()
            .method("GET")
            .uri("/api/v1/me")
            .body(Body::empty())
            .expect("valid request");

        let response = app.router.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
