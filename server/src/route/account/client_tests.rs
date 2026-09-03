use crate::auth::{encode_test_jwt, generate_test_keys, AuthClaims, JwksCache, OidcConfig};
use crate::handler::AppModule;
use crate::route::build_test_router_with_auth;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{
    AggregateRepository, AuthAccountRepository, AuthHostRepository, DependOnAccountRepository,
    DependOnAuthAccountRepository, DependOnAuthHostRepository,
    DependOnOrganizationMembershipRepository, OrganizationMembershipRepository,
};
use kernel::prelude::entity::{
    Account, AccountId, AccountIsBot, AccountName, AuthAccountId, AuthHostId, CreatedAt, Nanoid,
    OrgRole, OrganizationMembership, OrganizationMembershipStatus,
};
use kernel::test_utils::{AccountBuilder, AuthAccountBuilder, AuthHostBuilder};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const AUDIENCE: &str = "emumet";

struct Fixture {
    router: axum::Router,
    token: String,
    module: AppModule,
    organization: Account,
}

fn claims(issuer: &str, subject: &str) -> AuthClaims {
    AuthClaims {
        iss: issuer.to_string(),
        sub: subject.to_string(),
        aud: crate::auth::OneOrMany::One(AUDIENCE.to_string()),
        exp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_secs()
            + 3600,
    }
}

async fn fixture(keto_read_url: &str, membership: Option<OrgRole>) -> Fixture {
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
    .expect("AppModule init");
    let host_id = AuthHostId::default();
    let auth_account_id = AuthAccountId::default();
    let personal = AccountBuilder::new()
        .nanoid(Nanoid::new(format!("person-{}", uuid::Uuid::new_v4())))
        .build();
    let mut executor = module.database_connection().connection().await.unwrap();
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
        .unwrap();
    module
        .auth_account_repository()
        .create(
            &mut executor,
            &AuthAccountBuilder::new()
                .id(auth_account_id.clone())
                .host(host_id)
                .client_id(subject.clone())
                .build(),
        )
        .await
        .unwrap();
    module
        .account_read_model()
        .create(&mut executor, &personal)
        .await
        .unwrap();
    let organization_id = AccountId::default();
    let command = Account::create_organization(
        organization_id,
        AccountName::new(format!("org-{}", uuid::Uuid::new_v4())),
        AccountIsBot::new(false),
        Nanoid::new(format!("org-{}", uuid::Uuid::new_v4())),
        auth_account_id.clone(),
    );
    let event = module
        .account_repository()
        .save(&mut executor, command)
        .await
        .unwrap();
    let mut organization = None;
    Account::apply(&mut organization, event).unwrap();
    let organization = organization.unwrap();
    module
        .account_read_model()
        .create(&mut executor, &organization)
        .await
        .unwrap();
    module
        .account_read_model()
        .link_auth_account(&mut executor, personal.id(), &auth_account_id)
        .await
        .unwrap();
    if let Some(role) = membership {
        module
            .organization_membership_repository()
            .create(
                &mut executor,
                &OrganizationMembership::new(
                    organization.id().clone(),
                    personal.id().clone(),
                    role,
                    OrganizationMembershipStatus::Active,
                    personal.id().clone(),
                    CreatedAt::now(),
                ),
            )
            .await
            .unwrap();
    }
    let keys = generate_test_keys();
    let token = encode_test_jwt(&claims(&issuer, &subject), &keys.encoding_key, &keys.kid);
    let oidc_config = Arc::new(OidcConfig {
        issuer_url: issuer.clone(),
        expected_audience: AUDIENCE.to_string(),
        jwks_refetch_interval_secs: 0,
    });
    let jwks_cache = Arc::new(JwksCache::new_with_jwks(issuer, keys.jwk_set));
    Fixture {
        router: build_test_router_with_auth(module.clone(), oidc_config, jwks_cache),
        token,
        module,
        organization,
    }
}

fn request(
    method: &str,
    uri: &str,
    token: &str,
    organization: Option<&str>,
    body: Body,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    if let Some(organization) = organization {
        builder = builder.header("X-Organization-Id", organization);
    }
    builder.body(body).unwrap()
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn empty_keto() -> MockServer {
    let keto = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/relation-tuples"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "relation_tuples": [],
            "next_page_token": "",
        })))
        .mount(&keto)
        .await;
    keto
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn get_session_context_without_header_returns_personal_context_unchanged() {
    let keto = empty_keto().await;
    let fixture = fixture(&keto.uri(), None).await;
    let response = fixture
        .router
        .oneshot(request(
            "GET",
            "/api/v1/me/session-context",
            &fixture.token,
            None,
            Body::empty(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await["org_context"],
        serde_json::Value::Null
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn get_session_context_with_member_header_returns_organization_context() {
    let keto = empty_keto().await;
    let fixture = fixture(&keto.uri(), Some(OrgRole::Member)).await;
    let response = fixture
        .router
        .oneshot(request(
            "GET",
            "/api/v1/me/session-context",
            &fixture.token,
            Some(fixture.organization.nanoid().as_ref()),
            Body::empty(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response).await;
    assert_eq!(json["org_context"]["role"], "member");
    assert_eq!(
        json["org_context"]["org_account_id"].as_str(),
        Some(fixture.organization.nanoid().as_ref().as_str())
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn get_session_context_with_non_member_header_returns_forbidden() {
    let keto = empty_keto().await;
    let fixture = fixture(&keto.uri(), None).await;
    let response = fixture
        .router
        .oneshot(request(
            "GET",
            "/api/v1/me/session-context",
            &fixture.token,
            Some(fixture.organization.nanoid().as_ref()),
            Body::empty(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn get_session_context_with_nonexistent_organization_returns_not_found() {
    let keto = empty_keto().await;
    let fixture = fixture(&keto.uri(), None).await;
    let response = fixture
        .router
        .oneshot(request(
            "GET",
            "/api/v1/me/session-context",
            &fixture.token,
            Some("missing-organization"),
            Body::empty(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn create_profile_with_organization_context_uses_organization_account_id() {
    let keto = empty_keto().await;
    let fixture = fixture(&keto.uri(), Some(OrgRole::Member)).await;
    let response = fixture
        .router
        .oneshot(request(
            "POST",
            "/api/v1/profiles",
            &fixture.token,
            Some(fixture.organization.nanoid().as_ref()),
            Body::from(r#"{"display_name":"Org profile","summary":"Summary"}"#),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = response_json(response).await;
    assert_eq!(
        json["account_id"].as_str(),
        Some(fixture.organization.nanoid().as_ref().as_str())
    );
    let mut executor = fixture
        .module
        .database_connection()
        .connection()
        .await
        .unwrap();
    let profile = fixture
        .module
        .profile_read_model()
        .find_by_account_id(&mut executor, fixture.organization.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(profile.account_id(), fixture.organization.id());
}

/// profile-transfer (issue #61): orgs may hold multiple profiles after the
/// partial-unique migration; the old 1:1 application guard was removed.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn create_second_profile_for_same_organization_succeeds() {
    let keto = empty_keto().await;
    let fixture = fixture(&keto.uri(), Some(OrgRole::Member)).await;
    let organization_nanoid = fixture.organization.nanoid().as_ref().to_string();
    let first = request(
        "POST",
        "/api/v1/profiles",
        &fixture.token,
        Some(&organization_nanoid),
        Body::from(r#"{"display_name":"First"}"#),
    );
    let second = request(
        "POST",
        "/api/v1/profiles",
        &fixture.token,
        Some(&organization_nanoid),
        Body::from(r#"{"display_name":"Second"}"#),
    );
    let router = fixture.router;
    let first_response = router.clone().oneshot(first).await.unwrap();
    assert_eq!(first_response.status(), StatusCode::CREATED);
    let first_json = response_json(first_response).await;

    let second_response = router.oneshot(second).await.unwrap();

    assert_eq!(second_response.status(), StatusCode::CREATED);
    let second_json = response_json(second_response).await;
    assert_ne!(first_json["id"].as_str(), second_json["id"].as_str());
    assert_eq!(
        second_json["account_id"].as_str(),
        Some(organization_nanoid.as_str())
    );
    assert_eq!(second_json["display_name"].as_str(), Some("Second"));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn update_profile_with_organization_context_allows_active_member_without_keto_account_edit() {
    let keto = empty_keto().await;
    let fixture = fixture(&keto.uri(), Some(OrgRole::Member)).await;
    let organization_nanoid = fixture.organization.nanoid().as_ref().to_string();
    let router = fixture.router;
    let create_response = router
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/profiles",
            &fixture.token,
            Some(&organization_nanoid),
            Body::from(r#"{"display_name":"Before"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(create_response.status(), StatusCode::CREATED);

    let response = router
        .oneshot(request(
            "PATCH",
            &format!("/api/v1/accounts/{organization_nanoid}"),
            &fixture.token,
            Some(&organization_nanoid),
            Body::from(r#"{"display_name":"After"}"#),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["display_name"], "After");
}
