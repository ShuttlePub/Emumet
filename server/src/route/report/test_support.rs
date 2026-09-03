use crate::auth::{encode_test_jwt, generate_test_keys, AuthClaims, JwksCache, OidcConfig};
use crate::handler::AppModule;
use crate::route::build_test_router_with_auth;
use axum::body::Body;
use axum::http::{Method, Request};
use http_body_util::BodyExt;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event_store::{AccountEventStore, DependOnAccountEventStore};
use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
use kernel::interfaces::repository::{
    AuthAccountRepository, AuthHostRepository, DependOnAuthAccountRepository,
    DependOnAuthHostRepository,
};
use kernel::prelude::entity::{
    Account, AccountEvent, AuthAccountId, AuthHostId, CommandEnvelope, EventId, ExpectedVersion,
};
use kernel::test_utils::{AccountBuilder, AuthAccountBuilder, AuthHostBuilder};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const AUDIENCE: &str = "emumet";

pub(super) static REPORT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(super) struct ReportTestApp {
    pub router: axum::Router,
    pub token: String,
    pub target_nanoid: String,
    pub auth_account_id: AuthAccountId,
}

impl ReportTestApp {
    pub async fn new(keto: &MockServer) -> Self {
        kernel::ensure_generator_initialized();
        let issuer = format!("https://issuer.example/{}", uuid::Uuid::new_v4());
        let subject = format!("subject-{}", uuid::Uuid::new_v4());
        let module = AppModule::new_for_test_urls(
            "http://localhost:65535".to_string(),
            "http://localhost:65535".to_string(),
            keto.uri(),
            "http://localhost:65535".to_string(),
        )
        .await
        .expect("AppModule init failed (is DATABASE_URL set?)");

        let host_id = AuthHostId::default();
        let auth_account_id = AuthAccountId::default();
        let auth_host = AuthHostBuilder::new()
            .id(host_id.clone())
            .url(issuer.clone())
            .build();
        let auth_account = AuthAccountBuilder::new()
            .id(auth_account_id.clone())
            .host(host_id)
            .client_id(subject.clone())
            .build();
        let reporter = AccountBuilder::new().build();
        let target = AccountBuilder::new().build();
        let target_nanoid = target.nanoid().as_ref().to_string();
        let mut executor = module
            .database_connection()
            .connection()
            .await
            .expect("database executor");
        sqlx::query("TRUNCATE account_report_events, account_reports")
            .execute(&mut *executor)
            .await
            .expect("reset account reports");
        module
            .auth_host_repository()
            .create(&mut executor, &auth_host)
            .await
            .expect("seed auth host");
        module
            .auth_account_repository()
            .create(&mut executor, &auth_account)
            .await
            .expect("seed auth account");
        module
            .account_read_model()
            .create(&mut executor, &reporter)
            .await
            .expect("seed reporter account");
        module
            .account_read_model()
            .link_auth_account(&mut executor, reporter.id(), &auth_account_id)
            .await
            .expect("link reporter account");
        module
            .account_read_model()
            .create(&mut executor, &target)
            .await
            .expect("seed target account");
        let target_event = AccountEvent::Created {
            name: target.name().clone(),
            is_bot: target.is_bot().clone(),
            kind: target.kind().clone(),
            nanoid: target.nanoid().clone(),
            auth_account_id: auth_account_id.clone(),
        };
        module
            .account_event_store()
            .persist(
                &mut executor,
                &CommandEnvelope::<AccountEvent, Account>::new(
                    EventId::from(target.id().clone()),
                    target_event.name(),
                    target_event,
                    Some(ExpectedVersion::Nothing),
                ),
            )
            .await
            .expect("seed target account event");

        let keys = generate_test_keys();
        let token = encode_test_jwt(&claims(&issuer, &subject), &keys.encoding_key, &keys.kid);
        let oidc_config = Arc::new(OidcConfig {
            issuer_url: issuer.clone(),
            expected_audience: AUDIENCE.to_string(),
            jwks_refetch_interval_secs: 0,
        });
        let jwks_cache = Arc::new(JwksCache::new_with_jwks(issuer, keys.jwk_set));

        Self {
            router: build_test_router_with_auth(module, oidc_config, jwks_cache),
            token,
            target_nanoid,
            auth_account_id,
        }
    }

    pub fn json_request(
        &self,
        method: Method,
        uri: &str,
        body: serde_json::Value,
    ) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("valid request")
    }

    pub fn unauthenticated_request(method: Method, uri: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .expect("valid request")
    }

    pub async fn mock_moderator(&self, keto: &MockServer, allowed: bool, expected: u64) {
        Mock::given(method("POST"))
            .and(path("/relation-tuples/check"))
            .and(body_json(serde_json::json!({
                "namespace": "Instance",
                "object": "singleton",
                "relation": "moderate",
                "subject_id": self.auth_account_id.as_ref().to_string(),
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "allowed": allowed,
            })))
            .expect(expected)
            .mount(keto)
            .await;
    }
}

pub(super) async fn response_json(response: axum::response::Response) -> serde_json::Value {
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
