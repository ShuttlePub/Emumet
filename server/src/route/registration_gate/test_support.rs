use crate::auth::{encode_test_jwt, generate_test_keys, AuthClaims, JwksCache, OidcConfig};
use crate::handler::AppModule;
use crate::route::build_test_router_with_auth;
use application::service::registration_gate::RecordRegistrationObservationUseCase;
use axum::body::Body;
use axum::http::{Method, Request};
use http_body_util::BodyExt;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::repository::{
    AuthAccountRepository, AuthHostRepository, DependOnAuthAccountRepository,
    DependOnAuthHostRepository,
};
use kernel::prelude::entity::{
    AuthAccountId, AuthHostId, ProbeStatus, ProtectionObservation, RegistrationObservation,
};
use kernel::test_utils::{AuthAccountBuilder, AuthHostBuilder};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const AUDIENCE: &str = "emumet";

pub(crate) static GATE_ROUTE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) const HEALTHY: ProtectionObservation = ProtectionObservation {
    configured: true,
    probe: ProbeStatus::Healthy,
};
pub(crate) const DOWN: ProtectionObservation = ProtectionObservation {
    configured: true,
    probe: ProbeStatus::Unhealthy,
};
pub(crate) const OFF: ProtectionObservation = ProtectionObservation::UNCONFIGURED;

pub(crate) fn observation(
    email: ProtectionObservation,
    bot: ProtectionObservation,
) -> RegistrationObservation {
    RegistrationObservation::new(email, bot)
}

pub(crate) struct TestUser {
    pub token: String,
    pub auth_account_id: AuthAccountId,
}

pub(crate) struct GateTestApp {
    pub router: axum::Router,
    pub module: AppModule,
    pub admin_a: TestUser,
    pub admin_b: TestUser,
    pub outsider: TestUser,
}

impl GateTestApp {
    pub async fn new(keto: &MockServer) -> Self {
        Self::with_webhook_sender(keto, driver::webhook::HttpWebhookSender::disabled()).await
    }

    pub async fn with_webhook_sender(
        keto: &MockServer,
        webhook_sender: driver::webhook::HttpWebhookSender,
    ) -> Self {
        kernel::ensure_generator_initialized();
        let issuer = format!("https://issuer.example/{}", uuid::Uuid::new_v4());
        let module = AppModule::new_for_test_urls(
            "http://localhost:65535".to_string(),
            "http://localhost:65535".to_string(),
            keto.uri(),
            "http://localhost:65535".to_string(),
        )
        .await
        .expect("AppModule init failed (is DATABASE_URL set?)")
        .with_webhook_sender(webhook_sender);

        let mut executor = module
            .database_connection()
            .connection()
            .await
            .expect("database executor");
        sqlx::query(
            "TRUNCATE registration_gate_webhook_attempts, registration_gate_webhook_deliveries, \
             registration_gate_webhook_state, registration_gate_event_reads, \
             registration_gate_events, registration_gate_audit, registration_gate_state",
        )
        .execute(&mut *executor)
        .await
        .expect("reset registration gate");

        let host_id = AuthHostId::default();
        module
            .auth_host_repository()
            .create(
                &mut executor,
                &AuthHostBuilder::new()
                    .id(host_id.clone())
                    .url(issuer.clone())
                    .build(),
            )
            .await
            .expect("seed auth host");

        let keys = generate_test_keys();
        let mut users = Vec::new();
        for _ in 0..3 {
            let subject = format!("subject-{}", uuid::Uuid::new_v4());
            let auth_account_id = AuthAccountId::default();
            module
                .auth_account_repository()
                .create(
                    &mut executor,
                    &AuthAccountBuilder::new()
                        .id(auth_account_id.clone())
                        .host(host_id.clone())
                        .client_id(subject.clone())
                        .build(),
                )
                .await
                .expect("seed auth account");
            users.push(TestUser {
                token: encode_test_jwt(&claims(&issuer, &subject), &keys.encoding_key, &keys.kid),
                auth_account_id,
            });
        }
        drop(executor);

        let oidc_config = Arc::new(OidcConfig {
            issuer_url: issuer.clone(),
            expected_audience: AUDIENCE.to_string(),
            jwks_refetch_interval_secs: 0,
        });
        let jwks_cache = Arc::new(JwksCache::new_with_jwks(issuer, keys.jwk_set));
        let outsider = users.pop().unwrap();
        let admin_b = users.pop().unwrap();
        let admin_a = users.pop().unwrap();
        let app = Self {
            router: build_test_router_with_auth(module.clone(), oidc_config, jwks_cache),
            module,
            admin_a,
            admin_b,
            outsider,
        };
        app.mock_administrate(keto, &app.admin_a, true).await;
        app.mock_administrate(keto, &app.admin_b, true).await;
        app.mock_administrate(keto, &app.outsider, false).await;
        app
    }

    async fn mock_administrate(&self, keto: &MockServer, user: &TestUser, allowed: bool) {
        Mock::given(method("POST"))
            .and(path("/relation-tuples/check"))
            .and(body_json(serde_json::json!({
                "namespace": "Instance",
                "object": "singleton",
                "relation": "administrate",
                "subject_id": user.auth_account_id.as_ref().to_string(),
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "allowed": allowed,
            })))
            .mount(keto)
            .await;
    }

    pub async fn observe(&self, observation: RegistrationObservation) {
        self.module
            .record_registration_observation(observation)
            .await
            .expect("record observation");
    }
}

pub(crate) fn request(
    user: Option<&TestUser>,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(user) = user {
        builder = builder.header("authorization", format!("Bearer {}", user.token));
    }
    builder
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .expect("valid request")
}

pub(crate) async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let body = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes();
    serde_json::from_slice(&body).expect("JSON response")
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
