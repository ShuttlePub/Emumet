//! Real-database tests for the Profile domain-controls routes (D8P).
//!
//! Every test builds its own accounts / Profiles, so tests are independent and need no TRUNCATE.

use super::DomainControlRouter;
use crate::auth::{
    auth_middleware, encode_test_jwt, generate_test_keys, AuthClaims, JwksCache, OidcConfig,
};
use crate::handler::AppModule;
use crate::route::profile_transfer::ProfileTransferRouter;
use application::service::domain_control::FindDomainControlForActorUseCase;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{
    AggregateRepository, AuthAccountRepository, AuthHostRepository, DependOnAuthAccountRepository,
    DependOnAuthHostRepository, DependOnOrganizationMembershipRepository,
    DependOnProfileRepository, OrganizationMembershipRepository,
};
use kernel::prelude::entity::{
    Account, AccountKind, AuthAccountId, AuthHostId, CreatedAt, Nanoid, OrgRole,
    OrganizationMembership, OrganizationMembershipStatus, Profile, ProfileId,
};
use kernel::test_utils::{
    unique_account_name, AccountBuilder, AuthAccountBuilder, AuthHostBuilder,
};
use serde_json::{json, Value};
use sqlx::{Connection, PgConnection};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

const AUDIENCE: &str = "emumet";

struct User {
    token: String,
    personal: Account,
    profile: Profile,
}

/// Serializes the tests of this file: the lock tests below hold table / row locks that would
/// otherwise stall unrelated tests and make "who is blocked" ambiguous.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Harness {
    _serial: tokio::sync::MutexGuard<'static, ()>,
    module: AppModule,
    router: axum::Router,
    issuer: String,
    host_id: AuthHostId,
    keys: crate::auth::TestKeys,
}

impl Harness {
    async fn new() -> Self {
        let serial = SERIAL.lock().await;
        kernel::ensure_generator_initialized();
        let issuer = format!("https://issuer.example/{}", uuid::Uuid::new_v4());
        let module = AppModule::new_for_oauth2_test(
            "http://localhost:65535".to_string(),
            "http://localhost:65535".to_string(),
        )
        .await
        .expect("AppModule init failed (is DATABASE_URL set?)");
        let keys = generate_test_keys();
        let host_id = AuthHostId::default();
        let auth_host = AuthHostBuilder::new()
            .id(host_id.clone())
            .url(issuer.clone())
            .build();
        let mut executor = module.database_connection().connection().await.unwrap();
        module
            .auth_host_repository()
            .create(&mut executor, &auth_host)
            .await
            .expect("seed auth host");
        drop(executor);

        let oidc_config = Arc::new(OidcConfig {
            issuer_url: issuer.clone(),
            expected_audience: AUDIENCE.to_string(),
            jwks_refetch_interval_secs: 0,
        });
        let jwks_cache = Arc::new(JwksCache::new_with_jwks(
            issuer.clone(),
            keys.jwk_set.clone(),
        ));
        let api_v1 = axum::Router::new()
            .route_domain_controls()
            .route_profile_transfers();
        let router = axum::Router::new()
            .nest("/api/v1", api_v1)
            .layer(axum::middleware::from_fn_with_state(
                module.clone(),
                crate::api::organization_context::organization_context_middleware,
            ))
            .layer(axum::middleware::from_fn_with_state(
                (oidc_config, jwks_cache),
                auth_middleware,
            ))
            .with_state(module.clone());
        Self {
            _serial: serial,
            module,
            router,
            issuer,
            host_id,
            keys,
        }
    }

    fn token_for(&self, subject: &str) -> String {
        let claims = AuthClaims {
            iss: self.issuer.clone(),
            sub: subject.to_string(),
            aud: crate::auth::OneOrMany::One(AUDIENCE.to_string()),
            exp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock after Unix epoch")
                .as_secs()
                + 3600,
        };
        encode_test_jwt(&claims, &self.keys.encoding_key, &self.keys.kid)
    }

    async fn seed_profile(&self, account: &Account) -> Profile {
        let mut executor = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        let command = Profile::create(
            ProfileId::new(kernel::generate_id()),
            account.id().clone(),
            None,
            None,
            None,
            None,
            Nanoid::<Profile>::default(),
        );
        let event = self
            .module
            .profile_repository()
            .save(&mut executor, command)
            .await
            .expect("save profile event");
        let mut profile = None;
        Profile::apply(&mut profile, event).expect("apply profile event");
        let profile = profile.expect("profile");
        self.module
            .profile_read_model()
            .create(&mut executor, &profile)
            .await
            .expect("seed profile");
        profile
    }

    async fn seed_account(&self, kind: AccountKind) -> Account {
        let account = AccountBuilder::new()
            .kind(kind)
            .name(unique_account_name())
            .build();
        let mut executor = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        self.module
            .account_read_model()
            .create(&mut executor, &account)
            .await
            .expect("seed account");
        account
    }

    /// A signed-in person: auth account, personal Account and the personal Account's Profile.
    async fn user(&self) -> User {
        let subject = format!("subject-{}", uuid::Uuid::new_v4());
        let auth_account_id = AuthAccountId::default();
        let auth_account = AuthAccountBuilder::new()
            .id(auth_account_id.clone())
            .host(self.host_id.clone())
            .client_id(subject.clone())
            .build();
        let personal = self.seed_account(AccountKind::Personal).await;
        {
            let mut executor = self
                .module
                .database_connection()
                .connection()
                .await
                .unwrap();
            self.module
                .auth_account_repository()
                .create(&mut executor, &auth_account)
                .await
                .expect("seed auth account");
            self.module
                .account_read_model()
                .link_auth_account(&mut executor, personal.id(), &auth_account_id)
                .await
                .expect("link auth account");
        }
        let profile = self.seed_profile(&personal).await;
        User {
            token: self.token_for(&subject),
            personal,
            profile,
        }
    }

    async fn organization_with_member(&self, member: &User, role: OrgRole) -> Account {
        let org = self.seed_account(AccountKind::Organization).await;
        self.add_member(&org, member, role).await;
        org
    }

    async fn add_member(&self, org: &Account, member: &User, role: OrgRole) {
        let membership = OrganizationMembership::new(
            org.id().clone(),
            member.personal.id().clone(),
            role,
            OrganizationMembershipStatus::Active,
            member.personal.id().clone(),
            CreatedAt::now(),
        );
        let mut executor = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        self.module
            .organization_membership_repository()
            .create(&mut executor, &membership)
            .await
            .expect("seed membership");
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        organization: Option<&Account>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        send(&self.router, method, uri, token, organization, body).await
    }
}

async fn send(
    router: &axum::Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    organization: Option<&Account>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(organization) = organization {
        builder = builder.header("x-organization-id", organization.nanoid().as_ref());
    }
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("valid request");
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

fn controls_uri(profile: &Profile) -> String {
    format!(
        "/api/v1/profiles/{}/domain-controls",
        profile.nanoid().as_ref()
    )
}

fn control_uri(profile: &Profile, domain: &str) -> String {
    format!("{}/{domain}", controls_uri(profile))
}

fn items(value: &Value) -> Vec<(String, String, u64)> {
    value["items"]
        .as_array()
        .expect("items array")
        .iter()
        .map(|item| {
            (
                item["domain"].as_str().unwrap().to_string(),
                item["mode"].as_str().unwrap().to_string(),
                item["revision"].as_u64().unwrap(),
            )
        })
        .collect()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn crud_lifecycle_with_revisions() {
    let h = Harness::new().await;
    let owner = h.user().await;
    let token = Some(owner.token.as_str());
    let uri = control_uri(&owner.profile, "example.com");

    let (status, body) = h.call(Method::GET, &uri, token, None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no control yet: {body}");

    let (status, body) = h
        .call(
            Method::PUT,
            &uri,
            token,
            None,
            Some(json!({"mode": "block"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revision"], 1);
    assert_eq!(body["mode"], "block");
    assert_eq!(body["domain"], "example.com");
    assert_eq!(body["profile_id"], json!(owner.profile.nanoid().as_ref()));
    let created_at = body["created_at"].clone();

    let (status, body) = h.call(Method::GET, &uri, token, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (body["revision"].clone(), body["mode"].clone()),
        (json!(1), json!("block"))
    );

    // PUT without expected_revision on an existing control is a conflict and changes nothing.
    let (status, _) = h
        .call(
            Method::PUT,
            &uri,
            token,
            None,
            Some(json!({"mode": "mute"})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, body) = h.call(Method::GET, &uri, token, None, None).await;
    assert_eq!(
        (body["revision"].clone(), body["mode"].clone()),
        (json!(1), json!("block"))
    );

    // Stale / wrong expected_revision is a conflict.
    for wrong in [0u64, 2, 99] {
        let (status, _) = h
            .call(
                Method::PUT,
                &uri,
                token,
                None,
                Some(json!({"mode": "mute", "expected_revision": wrong})),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "expected_revision {wrong}");
    }

    // Replace with the matching revision: revision increases, created_at is kept.
    let (status, body) = h
        .call(
            Method::PUT,
            &uri,
            token,
            None,
            Some(json!({"mode": "mute", "expected_revision": 1})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (body["revision"].clone(), body["mode"].clone()),
        (json!(2), json!("mute"))
    );
    assert_eq!(body["created_at"], created_at);

    // The old revision can no longer be used.
    let (status, _) = h
        .call(
            Method::PUT,
            &uri,
            token,
            None,
            Some(json!({"mode": "block", "expected_revision": 1})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // expected_revision is required for DELETE (400), a wrong one conflicts (409).
    let (status, _) = h.call(Method::DELETE, &uri, token, None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = h
        .call(
            Method::DELETE,
            &format!("{uri}?expected_revision=1"),
            token,
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = h
        .call(
            Method::DELETE,
            &format!("{uri}?expected_revision=not-a-number"),
            token,
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = h.call(Method::GET, &uri, token, None, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "failed deletes must not remove the control"
    );

    let (status, _) = h
        .call(
            Method::DELETE,
            &format!("{uri}?expected_revision=2"),
            token,
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = h.call(Method::GET, &uri, token, None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = h
        .call(
            Method::DELETE,
            &format!("{uri}?expected_revision=2"),
            token,
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "deleting twice");
    let (_, listed) = h
        .call(
            Method::GET,
            &controls_uri(&owner.profile),
            token,
            None,
            None,
        )
        .await;
    assert!(items(&listed).is_empty(), "deleted controls are not listed");

    // A replace against a control deleted meanwhile is a conflict, not a create.
    let (status, _) = h
        .call(
            Method::PUT,
            &uri,
            token,
            None,
            Some(json!({"mode": "mute", "expected_revision": 2})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Re-creating continues the revision counter (never repeats), so stale tokens stay stale.
    let (status, body) = h
        .call(
            Method::PUT,
            &uri,
            token,
            None,
            Some(json!({"mode": "mute"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["revision"], 4,
        "create 1, replace 2, delete 3, re-create 4"
    );
    let (status, _) = h
        .call(
            Method::PUT,
            &uri,
            token,
            None,
            Some(json!({"mode": "block", "expected_revision": 2})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn domains_are_normalized_and_matched_exactly() {
    let h = Harness::new().await;
    let owner = h.user().await;
    let token = Some(owner.token.as_str());

    let (status, body) = h
        .call(
            Method::PUT,
            &control_uri(&owner.profile, "Example.COM."),
            token,
            None,
            Some(json!({"mode": "block"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["domain"], "example.com");

    // Every spelling of the same domain addresses the same control.
    for spelling in ["example.com", "EXAMPLE.com", "example.com."] {
        let (status, body) = h
            .call(
                Method::GET,
                &control_uri(&owner.profile, spelling),
                token,
                None,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{spelling}: {body}");
        assert_eq!(body["domain"], "example.com");
    }

    // Exact match: neither a subdomain nor the parent is covered.
    let (status, _) = h
        .call(
            Method::GET,
            &control_uri(&owner.profile, "sub.example.com"),
            token,
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = h
        .call(
            Method::PUT,
            &control_uri(&owner.profile, "sub.example.com"),
            token,
            None,
            Some(json!({"mode": "mute"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["revision"], 1,
        "the subdomain control is independent of the parent"
    );
    let (_, parent) = h
        .call(
            Method::GET,
            &control_uri(&owner.profile, "example.com"),
            token,
            None,
            None,
        )
        .await;
    assert_eq!(
        (parent["revision"].clone(), parent["mode"].clone()),
        (json!(1), json!("block"))
    );

    // IDN: percent-encoded U+00FC and the Punycode spelling are the same domain.
    let (status, body) = h
        .call(
            Method::PUT,
            &control_uri(&owner.profile, "b%C3%BCcher.example"),
            token,
            None,
            Some(json!({"mode": "mute"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["domain"], "xn--bcher-kva.example");
    let (status, body) = h
        .call(
            Method::GET,
            &control_uri(&owner.profile, "XN--BCHER-KVA.example"),
            token,
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["domain"], "xn--bcher-kva.example");

    let (_, listed) = h
        .call(
            Method::GET,
            &controls_uri(&owner.profile),
            token,
            None,
            None,
        )
        .await;
    assert_eq!(
        items(&listed),
        vec![
            ("example.com".to_string(), "block".to_string(), 1),
            ("sub.example.com".to_string(), "mute".to_string(), 1),
            ("xn--bcher-kva.example".to_string(), "mute".to_string(), 1),
        ]
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn malformed_domains_are_rejected_with_422() {
    let h = Harness::new().await;
    let owner = h.user().await;
    let token = Some(owner.token.as_str());

    for malformed in [
        "https%3A%2F%2Fexample.com",
        "example.com%3A8080",
        "example.com%2Fpath",
        "user%40example.com",
        "%2A.example.com",
        "127.0.0.1",
        "exa%20mple.com",
        "example.com..",
        "xn--",
        "ex%2561mple.com",
    ] {
        let uri = control_uri(&owner.profile, malformed);
        let (status, _) = h
            .call(
                Method::PUT,
                &uri,
                token,
                None,
                Some(json!({"mode": "block"})),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "PUT {malformed}");
        let (status, _) = h.call(Method::GET, &uri, token, None, None).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "GET {malformed}");
        let (status, _) = h
            .call(
                Method::DELETE,
                &format!("{uri}?expected_revision=1"),
                token,
                None,
                None,
            )
            .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "DELETE {malformed}"
        );
    }

    let (status, _) = h
        .call(
            Method::PUT,
            &control_uri(&owner.profile, "example.com"),
            token,
            None,
            Some(json!({"mode": "ban"})),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "unknown mode");
    let (_, listed) = h
        .call(
            Method::GET,
            &controls_uri(&owner.profile),
            token,
            None,
            None,
        )
        .await;
    assert!(items(&listed).is_empty(), "rejected requests store nothing");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn authentication_and_authorization() {
    let h = Harness::new().await;
    let owner = h.user().await;
    let stranger = h.user().await;
    let uri = control_uri(&owner.profile, "example.com");

    let (status, _) = h
        .call(
            Method::PUT,
            &uri,
            Some(owner.token.as_str()),
            None,
            Some(json!({"mode": "block"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // 401 without a token, for every operation.
    for (method, target, body) in [
        (Method::GET, controls_uri(&owner.profile), None),
        (Method::GET, uri.clone(), None),
        (Method::PUT, uri.clone(), Some(json!({"mode": "mute"}))),
        (Method::DELETE, format!("{uri}?expected_revision=1"), None),
    ] {
        let (status, _) = h.call(method.clone(), &target, None, None, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {target}");
    }

    // 403 for a signed-in non-owner, for every operation, and nothing changes.
    let stranger_token = Some(stranger.token.as_str());
    for (method, target, body) in [
        (Method::GET, controls_uri(&owner.profile), None),
        (Method::GET, uri.clone(), None),
        (
            Method::PUT,
            uri.clone(),
            Some(json!({"mode": "mute", "expected_revision": 1})),
        ),
        (
            Method::PUT,
            control_uri(&owner.profile, "other.example"),
            Some(json!({"mode": "mute"})),
        ),
        (Method::DELETE, format!("{uri}?expected_revision=1"), None),
    ] {
        let (status, _) = h
            .call(method.clone(), &target, stranger_token, None, body)
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {target}");
    }
    let (_, listed) = h
        .call(
            Method::GET,
            &controls_uri(&owner.profile),
            Some(owner.token.as_str()),
            None,
            None,
        )
        .await;
    assert_eq!(
        items(&listed),
        vec![("example.com".to_string(), "block".to_string(), 1)]
    );

    // 404 for a Profile that does not exist.
    let missing = "/api/v1/profiles/no-such-profile/domain-controls";
    for (method, target, body) in [
        (Method::GET, missing.to_string(), None),
        (Method::GET, format!("{missing}/example.com"), None),
        (
            Method::PUT,
            format!("{missing}/example.com"),
            Some(json!({"mode": "mute"})),
        ),
        (
            Method::DELETE,
            format!("{missing}/example.com?expected_revision=1"),
            None,
        ),
    ] {
        let (status, _) = h
            .call(
                method.clone(),
                &target,
                Some(owner.token.as_str()),
                None,
                body,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {target}");
    }

    // The owner's personal Profile cannot be reached through an organization context.
    let org = h.organization_with_member(&owner, OrgRole::Owner).await;
    let (status, _) = h
        .call(
            Method::GET,
            &uri,
            Some(owner.token.as_str()),
            Some(&org),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // And an organization context for an organization the caller does not belong to is refused.
    let (status, _) = h
        .call(
            Method::GET,
            &uri,
            Some(stranger.token.as_str()),
            Some(&org),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn controls_are_isolated_per_profile() {
    let h = Harness::new().await;
    let alice = h.user().await;
    let bob = h.user().await;

    // Another Account's Profile.
    let (status, _) = h
        .call(
            Method::PUT,
            &control_uri(&alice.profile, "example.com"),
            Some(alice.token.as_str()),
            None,
            Some(json!({"mode": "block"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, bob_list) = h
        .call(
            Method::GET,
            &controls_uri(&bob.profile),
            Some(bob.token.as_str()),
            None,
            None,
        )
        .await;
    assert!(items(&bob_list).is_empty());
    let (status, _) = h
        .call(
            Method::GET,
            &control_uri(&bob.profile, "example.com"),
            Some(bob.token.as_str()),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Bob can create the same domain for his own Profile independently (revision 1).
    let (status, body) = h
        .call(
            Method::PUT,
            &control_uri(&bob.profile, "example.com"),
            Some(bob.token.as_str()),
            None,
            Some(json!({"mode": "mute"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (body["revision"].clone(), body["mode"].clone()),
        (json!(1), json!("mute"))
    );
    let (_, alice_control) = h
        .call(
            Method::GET,
            &control_uri(&alice.profile, "example.com"),
            Some(alice.token.as_str()),
            None,
            None,
        )
        .await;
    assert_eq!(
        (
            alice_control["revision"].clone(),
            alice_control["mode"].clone()
        ),
        (json!(1), json!("block"))
    );

    // Two Profiles of the same (organization) Account are independent as well.
    let org = h.organization_with_member(&alice, OrgRole::Member).await;
    let first = h.seed_profile(&org).await;
    let second = h.seed_profile(&org).await;
    let (status, _) = h
        .call(
            Method::PUT,
            &control_uri(&first, "shared.example"),
            Some(alice.token.as_str()),
            Some(&org),
            Some(json!({"mode": "block"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, second_list) = h
        .call(
            Method::GET,
            &controls_uri(&second),
            Some(alice.token.as_str()),
            Some(&org),
            None,
        )
        .await;
    assert!(
        items(&second_list).is_empty(),
        "the sibling Profile is unaffected"
    );
    let (_, first_list) = h
        .call(
            Method::GET,
            &controls_uri(&first),
            Some(alice.token.as_str()),
            Some(&org),
            None,
        )
        .await;
    assert_eq!(items(&first_list).len(), 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn after_profile_transfer_only_the_new_owner_operates_and_controls_stay() {
    let h = Harness::new().await;
    let alice = h.user().await;
    let org_member = h.user().await;
    let outsider = h.user().await;
    let org = h.organization_with_member(&alice, OrgRole::Owner).await;
    h.add_member(&org, &org_member, OrgRole::Member).await;
    let alice_token = Some(alice.token.as_str());

    let uri = control_uri(&alice.profile, "example.com");
    let (status, _) = h
        .call(
            Method::PUT,
            &uri,
            alice_token,
            None,
            Some(json!({"mode": "block"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = h
        .call(
            Method::PUT,
            &uri,
            alice_token,
            None,
            Some(json!({"mode": "mute", "expected_revision": 1})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Real transfer: Alice requests (she is an active member of the organization) and accepts.
    let (status, body) = h
        .call(
            Method::POST,
            &format!(
                "/api/v1/profiles/{}/transfer-requests",
                alice.profile.nanoid().as_ref()
            ),
            alice_token,
            None,
            Some(json!({"org_account_nanoid": org.nanoid().as_ref()})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "transfer request: {body}");
    let request_id = body["nanoid"]
        .as_str()
        .expect("transfer request id")
        .to_string();
    let (status, body) = h
        .call(
            Method::POST,
            &format!("/api/v1/profile-transfer-requests/{request_id}/accept"),
            alice_token,
            None,
            None,
        )
        .await;
    assert!(status.is_success(), "transfer accept: {status} {body}");

    // The previous owner (personal Account) is refused for every operation.
    for (method, target, body) in [
        (Method::GET, controls_uri(&alice.profile), None),
        (Method::GET, uri.clone(), None),
        (
            Method::PUT,
            uri.clone(),
            Some(json!({"mode": "block", "expected_revision": 2})),
        ),
        (
            Method::PUT,
            control_uri(&alice.profile, "new.example"),
            Some(json!({"mode": "mute"})),
        ),
        (Method::DELETE, format!("{uri}?expected_revision=2"), None),
    ] {
        let (status, _) = h
            .call(method.clone(), &target, alice_token, None, body)
            .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "previous owner: {method} {target}"
        );
    }
    // A non-member of the new owning organization is refused too.
    let (status, _) = h
        .call(
            Method::GET,
            &uri,
            Some(outsider.token.as_str()),
            Some(&org),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The new owner (organization context) sees the same control: policy stays on the Profile.
    let (status, body) = h
        .call(Method::GET, &uri, alice_token, Some(&org), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (body["revision"].clone(), body["mode"].clone()),
        (json!(2), json!("mute"))
    );
    let member_token = Some(org_member.token.as_str());
    let (status, body) = h
        .call(
            Method::PUT,
            &uri,
            member_token,
            Some(&org),
            Some(json!({"mode": "block", "expected_revision": 2})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (body["revision"].clone(), body["mode"].clone()),
        (json!(3), json!("block"))
    );
    let (status, _) = h
        .call(
            Method::DELETE,
            &format!("{uri}?expected_revision=3"),
            alice_token,
            Some(&org),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_writers_exactly_one_wins() {
    let h = Harness::new().await;
    let owner = h.user().await;
    let uri = control_uri(&owner.profile, "race.example");
    const WRITERS: usize = 12;

    let run = |method: Method, uri: String, body: Option<Value>| {
        let handles: Vec<_> = (0..WRITERS)
            .map(|_| {
                let router = h.router.clone();
                let token = owner.token.clone();
                let method = method.clone();
                let uri = uri.clone();
                let body = body.clone();
                tokio::spawn(async move {
                    send(&router, method, &uri, Some(&token), None, body)
                        .await
                        .0
                })
            })
            .collect();
        async move {
            let mut statuses = Vec::new();
            for handle in handles {
                statuses.push(handle.await.unwrap());
            }
            statuses
        }
    };
    let count = |statuses: &[StatusCode], wanted: StatusCode| {
        statuses.iter().filter(|status| **status == wanted).count()
    };

    // Concurrent creates: one 200, the rest 409.
    let statuses = run(Method::PUT, uri.clone(), Some(json!({"mode": "block"}))).await;
    assert_eq!(count(&statuses, StatusCode::OK), 1, "{statuses:?}");
    assert_eq!(
        count(&statuses, StatusCode::CONFLICT),
        WRITERS - 1,
        "{statuses:?}"
    );

    // Concurrent replaces against the same revision: one 200, the rest 409, revision 2.
    let statuses = run(
        Method::PUT,
        uri.clone(),
        Some(json!({"mode": "mute", "expected_revision": 1})),
    )
    .await;
    assert_eq!(count(&statuses, StatusCode::OK), 1, "{statuses:?}");
    assert_eq!(
        count(&statuses, StatusCode::CONFLICT),
        WRITERS - 1,
        "{statuses:?}"
    );
    let (_, body) = h
        .call(Method::GET, &uri, Some(owner.token.as_str()), None, None)
        .await;
    assert_eq!(
        (body["revision"].clone(), body["mode"].clone()),
        (json!(2), json!("mute"))
    );

    // Concurrent deletes of the same revision: one 204, the others find nothing (404) or conflict.
    let statuses = run(Method::DELETE, format!("{uri}?expected_revision=2"), None).await;
    assert_eq!(count(&statuses, StatusCode::NO_CONTENT), 1, "{statuses:?}");
    assert!(
        statuses.iter().all(|s| matches!(
            *s,
            StatusCode::NO_CONTENT | StatusCode::NOT_FOUND | StatusCode::CONFLICT
        )),
        "{statuses:?}"
    );
    let (status, _) = h
        .call(Method::GET, &uri, Some(owner.token.as_str()), None, None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn matching_uses_the_verified_actor_id_host_only() {
    let h = Harness::new().await;
    let alice = h.user().await;
    let bob = h.user().await;
    let (status, _) = h
        .call(
            Method::PUT,
            &control_uri(&alice.profile, "example.com"),
            Some(alice.token.as_str()),
            None,
            Some(json!({"mode": "block"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let found = |profile: &Profile, actor_id: &'static str| {
        let module = h.module.clone();
        let profile_id = profile.id().clone();
        async move {
            module
                .find_domain_control_for_actor(&profile_id, actor_id)
                .await
                .expect("lookup")
        }
    };

    let hit = found(&alice.profile, "https://example.com/users/mallory").await;
    assert_eq!(
        hit.map(|c| (c.revision(), c.domain().to_string())),
        Some((1, "example.com".to_string()))
    );
    for same_domain in [
        "https://EXAMPLE.com/users/mallory",
        "https://example.com:8443/users/mallory",
        "https://example.com./users/mallory",
    ] {
        assert!(
            found(&alice.profile, same_domain).await.is_some(),
            "{same_domain}"
        );
    }
    // Exact match: subdomains and look-alikes do not match.
    for other in [
        "https://sub.example.com/users/mallory",
        "https://notexample.com/users/mallory",
        "https://example.com.evil.test/users/mallory",
        "https://evil.test/users/example.com",
        "https://good.example@evil.test/users/mallory",
        "https://127.0.0.1/users/mallory",
        "not a url",
    ] {
        assert!(found(&alice.profile, other).await.is_none(), "{other}");
    }
    // The lookup key is the actor ID: an actor whose inbox / CDN lives on example.com but whose
    // ID is elsewhere is not matched, because only the ID is ever passed in.
    assert!(
        found(&alice.profile, "https://actors.other.test/users/cdn-user")
            .await
            .is_none()
    );
    // Another Profile has no such control.
    assert!(found(&bob.profile, "https://example.com/users/mallory")
        .await
        .is_none());
}

async fn raw_connection() -> PgConnection {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    PgConnection::connect(&url).await.expect("raw connection")
}

async fn backend_pid(connection: &mut PgConnection) -> i32 {
    sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()")
        .fetch_one(connection)
        .await
        .unwrap()
}

/// Waits until some backend is blocked behind `blocker_pid` (the tests of this file are
/// serialized, so that backend is the request under test).
async fn wait_until_blocked_by(observer: &mut PgConnection, blocker_pid: i32) {
    for _ in 0..200 {
        let blocked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))",
        )
        .bind(blocker_pid)
        .fetch_one(&mut *observer)
        .await
        .unwrap();
        if blocked > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("no request became blocked behind backend {blocker_pid}");
}

#[derive(Clone, Copy, Debug)]
enum Operation {
    List,
    Get,
    Put,
    Delete,
}

impl Operation {
    const ALL: [Operation; 4] = [
        Operation::List,
        Operation::Get,
        Operation::Put,
        Operation::Delete,
    ];

    fn ok_status(self) -> StatusCode {
        match self {
            Operation::Delete => StatusCode::NO_CONTENT,
            _ => StatusCode::OK,
        }
    }

    /// Request of this operation against a control that exists with revision 1 (`locked.example`),
    /// except PUT which creates a new one (`created.example`).
    fn request(self, profile: &Profile) -> (Method, String, Option<Value>) {
        match self {
            Operation::List => (Method::GET, controls_uri(profile), None),
            Operation::Get => (Method::GET, control_uri(profile, "locked.example"), None),
            Operation::Put => (
                Method::PUT,
                control_uri(profile, "created.example"),
                Some(json!({"mode": "block"})),
            ),
            Operation::Delete => (
                Method::DELETE,
                format!(
                    "{}?expected_revision=1",
                    control_uri(profile, "locked.example")
                ),
                None,
            ),
        }
    }
}

async fn seed_locked_control(h: &Harness, owner: &User) {
    let (status, body) = h
        .call(
            Method::PUT,
            &control_uri(&owner.profile, "locked.example"),
            Some(owner.token.as_str()),
            None,
            Some(json!({"mode": "mute"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A Profile transfer updates the Profile row. While an authorized operation is in flight (here:
/// stuck on a table lock AFTER the owner check) that update must wait; once the operation is
/// done the transfer goes through and the previous owner is refused.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_authorized_operation_in_flight_holds_the_profile_against_transfer() {
    let h = Harness::new().await;
    for operation in Operation::ALL {
        let owner = h.user().await;
        let org = h.organization_with_member(&owner, OrgRole::Owner).await;
        seed_locked_control(&h, &owner).await;

        let mut blocker = raw_connection().await;
        let blocker_pid = backend_pid(&mut blocker).await;
        sqlx::query("BEGIN").execute(&mut blocker).await.unwrap();
        sqlx::query("LOCK TABLE profile_domain_controls IN ACCESS EXCLUSIVE MODE")
            .execute(&mut blocker)
            .await
            .unwrap();

        let (method, uri, body) = operation.request(&owner.profile);
        let router = h.router.clone();
        let token = owner.token.clone();
        let in_flight = tokio::spawn(async move {
            send(&router, method, &uri, Some(&token), None, body)
                .await
                .0
        });
        let mut observer = raw_connection().await;
        wait_until_blocked_by(&mut observer, blocker_pid).await;
        assert!(
            !in_flight.is_finished(),
            "{operation:?} must be stuck behind the table lock"
        );

        // The transfer's UPDATE of the Profile row must NOT get through while the operation holds
        // the Profile.
        let mut transfer = raw_connection().await;
        sqlx::query("SET lock_timeout = '300ms'")
            .execute(&mut transfer)
            .await
            .unwrap();
        let blocked = sqlx::query(
            "UPDATE profiles SET account_id = $2,
               owner_kind = (SELECT kind FROM accounts WHERE id = $2) WHERE id = $1",
        )
        .bind(owner.profile.id().as_ref())
        .bind(org.id().as_ref())
        .execute(&mut transfer)
        .await;
        let error = blocked.expect_err("the transfer update must wait for the in-flight operation");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("55P03"),
            "{operation:?}: expected lock_not_available, got {error}"
        );

        sqlx::query("ROLLBACK").execute(&mut blocker).await.unwrap();
        let status = in_flight.await.unwrap();
        assert_eq!(
            status,
            operation.ok_status(),
            "{operation:?} was authorized before the transfer"
        );

        sqlx::query("SET lock_timeout = 0")
            .execute(&mut transfer)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE profiles SET account_id = $2,
               owner_kind = (SELECT kind FROM accounts WHERE id = $2) WHERE id = $1",
        )
        .bind(owner.profile.id().as_ref())
        .bind(org.id().as_ref())
        .execute(&mut transfer)
        .await
        .expect("the transfer proceeds once the operation finished");

        let (method, uri, body) = operation.request(&owner.profile);
        let (status, _) = h
            .call(method, &uri, Some(owner.token.as_str()), None, body)
            .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{operation:?}: previous owner after the transfer"
        );
    }
}

/// A transfer that is in flight (updated the Profile row, not yet committed) is waited for by the
/// owner check, and the previous owner is refused with the committed result.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn owner_check_waits_for_an_in_flight_transfer_and_rejects_the_previous_owner() {
    let h = Harness::new().await;
    for operation in Operation::ALL {
        let owner = h.user().await;
        let org = h.organization_with_member(&owner, OrgRole::Owner).await;
        seed_locked_control(&h, &owner).await;

        let mut transfer = raw_connection().await;
        let transfer_pid = backend_pid(&mut transfer).await;
        sqlx::query("BEGIN").execute(&mut transfer).await.unwrap();
        sqlx::query(
            "UPDATE profiles SET account_id = $2,
               owner_kind = (SELECT kind FROM accounts WHERE id = $2) WHERE id = $1",
        )
        .bind(owner.profile.id().as_ref())
        .bind(org.id().as_ref())
        .execute(&mut transfer)
        .await
        .unwrap();

        let (method, uri, body) = operation.request(&owner.profile);
        let router = h.router.clone();
        let token = owner.token.clone();
        let previous_owner = tokio::spawn(async move {
            send(&router, method, &uri, Some(&token), None, body)
                .await
                .0
        });
        let mut observer = raw_connection().await;
        wait_until_blocked_by(&mut observer, transfer_pid).await;
        assert!(
            !previous_owner.is_finished(),
            "{operation:?}: the owner check must wait for the uncommitted transfer"
        );

        sqlx::query("COMMIT").execute(&mut transfer).await.unwrap();
        assert_eq!(
            previous_owner.await.unwrap(),
            StatusCode::FORBIDDEN,
            "{operation:?}: the previous owner must be refused once the transfer committed"
        );

        let (method, uri, body) = operation.request(&owner.profile);
        let (status, body_json) = send(
            &h.router,
            method,
            &uri,
            Some(owner.token.as_str()),
            Some(&org),
            body,
        )
        .await;
        assert_eq!(
            status,
            operation.ok_status(),
            "{operation:?}: new owner: {body_json}"
        );
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn domain_control_routes_are_mounted_on_the_production_shaped_router() {
    let module = AppModule::new_for_oauth2_test(
        "http://localhost:65535".to_string(),
        "http://localhost:65535".to_string(),
    )
    .await
    .expect("AppModule init failed (is DATABASE_URL set?)");
    let router = crate::route::build_test_router(module);

    let spec: Value = serde_json::from_str(&crate::openapi::generate_openapi_json()).unwrap();
    let mut checked = 0;
    for (template, item) in spec["paths"].as_object().unwrap() {
        if !template.contains("/domain-controls") {
            continue;
        }
        for method in item.as_object().unwrap().keys() {
            let method = Method::from_bytes(method.to_uppercase().as_bytes()).unwrap();
            let uri = template
                .replace("{profile_nanoid}", "profile")
                .replace("{domain}", "example.com");
            let (status, _) = send(&router, method.clone(), &uri, None, None, None).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{method} {uri} must be routed (and authenticated) in the production-shaped router"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 4, "list, get, put and delete");
}
