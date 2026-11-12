//! P1: Profile username storage through the REST API (real database, real use cases; only Keto
//! is mocked). Covers legacy and new payloads, duplicate / reserved / malformed rejection with
//! no side effects, and the one-time setter.

use super::{
    count_rows, ensure_master_key_password_file, fixture, fixture_with_keto_write,
    keto_with_writes, request, response_json, Fixture,
};
use crate::handler::AppModule;
use axum::body::Body;
use axum::http::StatusCode;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::prelude::entity::OrgRole;
use tower::ServiceExt;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn unique_username() -> String {
    format!("u{}", uuid::Uuid::new_v4().simple())
}

async fn scalar_string(module: &AppModule, sql: &str, bind: &str) -> Option<String> {
    let mut executor = module.database_connection().connection().await.unwrap();
    let con: &mut sqlx::PgConnection = &mut executor;
    sqlx::query_scalar::<_, Option<String>>(sql)
        .bind(bind)
        .fetch_optional(con)
        .await
        .unwrap()
        .flatten()
}

/// Turns a Profile into the pre-cutover shape (no username anywhere: neither in the projection nor
/// in its events), so the one-time setter can be exercised. Every Profile created through the API
/// has a username since the P4 cutover.
async fn strip_profile_username(module: &AppModule, profile_nanoid: &str) {
    let mut executor = module.database_connection().connection().await.unwrap();
    let con: &mut sqlx::PgConnection = &mut executor;
    sqlx::query(
        "UPDATE profile_events SET data = data - 'username' \
         WHERE id = (SELECT id FROM profiles WHERE nanoid = $1)",
    )
    .bind(profile_nanoid)
    .execute(&mut *con)
    .await
    .unwrap();
    sqlx::query("UPDATE profiles SET username = NULL WHERE nanoid = $1")
        .bind(profile_nanoid)
        .execute(con)
        .await
        .unwrap();
}

async fn scalar_i64(module: &AppModule, sql: &str, bind: &str) -> i64 {
    let mut executor = module.database_connection().connection().await.unwrap();
    let con: &mut sqlx::PgConnection = &mut executor;
    sqlx::query_scalar::<_, i64>(sql)
        .bind(bind)
        .fetch_one(con)
        .await
        .unwrap()
}

async fn username_event_count(module: &AppModule, username: &str) -> i64 {
    count_rows(
        module,
        "SELECT count(*) FROM profile_events WHERE data ->> 'username' = $1",
        username,
    )
    .await
}

async fn post_account(
    fixture: &Fixture,
    token: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/accounts",
            token,
            None,
            Body::from(body.to_string()),
        ))
        .await
        .unwrap()
}

async fn post_username(
    fixture: &Fixture,
    token: &str,
    organization: Option<&str>,
    profile_nanoid: &str,
    username: &str,
) -> axum::response::Response {
    fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/profiles/{profile_nanoid}/username"),
            token,
            organization,
            Body::from(serde_json::json!({ "username": username }).to_string()),
        ))
        .await
        .unwrap()
}

/// Keto mock where only `allowed_subject` passes permission checks (everybody else gets the
/// "not allowed" 403 Keto v0.12 answers with).
async fn mount_keto_allowing_only(keto: &MockServer, allowed_subject: &str) {
    Mock::given(method("GET"))
        .and(path("/relation-tuples"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "relation_tuples": [],
            "next_page_token": "",
        })))
        .mount(keto)
        .await;
    Mock::given(method("PUT"))
        .and(path("/admin/relation-tuples"))
        .respond_with(ResponseTemplate::new(201))
        .mount(keto)
        .await;
    Mock::given(method("POST"))
        .and(path("/relation-tuples/check"))
        .and(body_partial_json(
            serde_json::json!({ "subject_id": allowed_subject }),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"allowed": true})),
        )
        .with_priority(1)
        .mount(keto)
        .await;
    Mock::given(method("POST"))
        .and(path("/relation-tuples/check"))
        .respond_with(ResponseTemplate::new(403))
        .with_priority(5)
        .mount(keto)
        .await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn create_account_with_valid_username_stores_it_and_rejects_a_duplicate() {
    ensure_master_key_password_file();
    let keto = keto_with_writes().await;
    let fixture = fixture_with_keto_write(&keto.uri(), &keto.uri(), None).await;
    let username = unique_username();
    let name = format!("stored-username-{}", uuid::Uuid::new_v4());

    let response = post_account(
        &fixture,
        &fixture.token,
        serde_json::json!({"name": name, "is_bot": false, "profile_username": username}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        scalar_string(
            &fixture.module,
            "SELECT p.username FROM profiles p JOIN accounts a ON a.id = p.account_id WHERE a.name = $1",
            &name,
        )
        .await
        .as_deref(),
        Some(username.as_str()),
        "the username must be stored on the Profile created with the Account"
    );
    assert_eq!(username_event_count(&fixture.module, &username).await, 1);

    let other_name = format!("duplicate-username-{}", uuid::Uuid::new_v4());
    let response = post_account(
        &fixture,
        &fixture.token,
        serde_json::json!({"name": other_name, "is_bot": false, "profile_username": username}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        count_rows(
            &fixture.module,
            "SELECT count(*) FROM accounts WHERE name = $1",
            &other_name,
        )
        .await,
        0,
        "a duplicate username must roll the whole creation back"
    );
    assert_eq!(
        username_event_count(&fixture.module, &username).await,
        1,
        "a rejected duplicate must not leave an event behind"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn create_profile_with_username_stores_and_returns_it() {
    let keto = super::empty_keto().await;
    let fixture = fixture(&keto.uri(), Some(OrgRole::Member)).await;
    let organization_nanoid = fixture.organization.nanoid().as_ref().to_string();
    let username = unique_username();

    let response = fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/profiles",
            &fixture.token,
            Some(&organization_nanoid),
            Body::from(serde_json::json!({"username": username}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let json = response_json(response).await;
    assert_eq!(json["username"], username.as_str());
    assert_eq!(username_event_count(&fixture.module, &username).await, 1);

    // Duplicate, malformed and reserved values are rejected and create nothing.
    let profiles_before = scalar_i64(
        &fixture.module,
        "SELECT count(*) FROM profiles WHERE account_id = (SELECT id FROM accounts WHERE nanoid = $1)",
        &organization_nanoid,
    )
    .await;
    for (value, expected) in [
        (username.as_str(), StatusCode::CONFLICT),
        ("Alice", StatusCode::BAD_REQUEST),
        ("root", StatusCode::BAD_REQUEST),
    ] {
        let response = fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                "/api/v1/profiles",
                &fixture.token,
                Some(&organization_nanoid),
                Body::from(serde_json::json!({"username": value}).to_string()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "username {value:?}");
    }
    assert_eq!(
        scalar_i64(
            &fixture.module,
            "SELECT count(*) FROM profiles WHERE account_id = (SELECT id FROM accounts WHERE nanoid = $1)",
            &organization_nanoid,
        )
        .await,
        profiles_before,
        "rejected usernames must not create a Profile"
    );

    // The username is required (P4): omitted or null is a validation error and creates nothing.
    for body in [
        r#"{"display_name":"No username"}"#,
        r#"{"display_name":"Null username","username":null}"#,
    ] {
        let response = fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                "/api/v1/profiles",
                &fixture.token,
                Some(&organization_nanoid),
                Body::from(body),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
    }
    assert_eq!(
        scalar_i64(
            &fixture.module,
            "SELECT count(*) FROM profiles WHERE account_id = (SELECT id FROM accounts WHERE nanoid = $1)",
            &organization_nanoid,
        )
        .await,
        profiles_before,
        "a missing username must not create a Profile"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn owner_sets_username_once_and_cannot_change_it() {
    ensure_master_key_password_file();
    // Bootstrap with an allow-all Keto to create the Account, then restrict permission checks to
    // the owner's auth account.
    let keto = keto_with_writes().await;
    let fixture = fixture_with_keto_write(&keto.uri(), &keto.uri(), None).await;
    let name = format!("set-once-{}", uuid::Uuid::new_v4());
    let response = post_account(
        &fixture,
        &fixture.token,
        serde_json::json!({"name": name, "is_bot": false, "profile_username": unique_username()}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let profile_nanoid = scalar_string(
        &fixture.module,
        "SELECT p.nanoid FROM profiles p JOIN accounts a ON a.id = p.account_id WHERE a.name = $1",
        &name,
    )
    .await
    .unwrap();
    strip_profile_username(&fixture.module, &profile_nanoid).await;
    let owner_auth_id = scalar_i64(
        &fixture.module,
        "SELECT auth_id FROM auth_emumet_accounts WHERE emumet_id = (SELECT id FROM accounts WHERE name = $1)",
        &name,
    )
    .await
    .to_string();
    keto.reset().await;
    mount_keto_allowing_only(&keto, &owner_auth_id).await;

    let username = unique_username();

    // A different authenticated user is not the owner.
    let response = post_username(
        &fixture,
        &fixture.fresh_token,
        None,
        &profile_nanoid,
        &username,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(username_event_count(&fixture.module, &username).await, 0);

    // Malformed / reserved values are 400, unknown Profiles 404.
    for bad in ["Alice", " alice", "admin", ""] {
        let response = post_username(&fixture, &fixture.token, None, &profile_nanoid, bad).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{bad:?}");
    }
    let response =
        post_username(&fixture, &fixture.token, None, "no-such-profile", &username).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // The owner sets it once and gets the Profile back.
    let response = post_username(&fixture, &fixture.token, None, &profile_nanoid, &username).await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = response_json(response).await;
    assert_eq!(json["username"], username.as_str());
    assert_eq!(json["id"], profile_nanoid.as_str());
    assert_eq!(
        scalar_string(
            &fixture.module,
            "SELECT username FROM profiles WHERE nanoid = $1",
            &profile_nanoid,
        )
        .await
        .as_deref(),
        Some(username.as_str())
    );

    // Neither the same nor another value can be set again.
    for again in [username.clone(), unique_username()] {
        let response = post_username(&fixture, &fixture.token, None, &profile_nanoid, &again).await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{again}");
    }
    assert_eq!(
        scalar_string(
            &fixture.module,
            "SELECT username FROM profiles WHERE nanoid = $1",
            &profile_nanoid,
        )
        .await
        .as_deref(),
        Some(username.as_str()),
        "the username is immutable"
    );

    // A username already used by another Profile is a conflict as well.
    let name2 = format!("set-once-2-{}", uuid::Uuid::new_v4());
    let response = post_account(
        &fixture,
        &fixture.token,
        serde_json::json!({"name": name2, "is_bot": false, "profile_username": unique_username()}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let profile2 = scalar_string(
        &fixture.module,
        "SELECT p.nanoid FROM profiles p JOIN accounts a ON a.id = p.account_id WHERE a.name = $1",
        &name2,
    )
    .await
    .unwrap();
    strip_profile_username(&fixture.module, &profile2).await;
    let response = post_username(&fixture, &fixture.token, None, &profile2, &username).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(scalar_string(
        &fixture.module,
        "SELECT username FROM profiles WHERE nanoid = $1",
        &profile2,
    )
    .await
    .is_none());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn organization_member_sets_the_username_of_an_organization_profile() {
    let keto = super::empty_keto().await;
    mount_keto_allowing_only(&keto, "nobody").await;
    let fixture = fixture(&keto.uri(), Some(OrgRole::Member)).await;
    let organization_nanoid = fixture.organization.nanoid().as_ref().to_string();
    let response = fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/profiles",
            &fixture.token,
            Some(&organization_nanoid),
            Body::from(
                serde_json::json!({"display_name":"No username yet","username":unique_username()})
                    .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let profile_nanoid = response_json(response).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    strip_profile_username(&fixture.module, &profile_nanoid).await;
    let username = unique_username();

    // Without the organization header the caller acts as a person (Keto denies everybody here).
    let response = post_username(&fixture, &fixture.token, None, &profile_nanoid, &username).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = post_username(
        &fixture,
        &fixture.token,
        Some(&organization_nanoid),
        &profile_nanoid,
        &username,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["username"], username.as_str());

    let response = post_username(
        &fixture,
        &fixture.token,
        Some(&organization_nanoid),
        &profile_nanoid,
        &unique_username(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_assignments_of_one_username_let_exactly_one_win() {
    ensure_master_key_password_file();
    let keto = keto_with_writes().await;
    let fixture = fixture_with_keto_write(&keto.uri(), &keto.uri(), None).await;
    let username = unique_username();
    let mut handles = Vec::new();
    for index in 0..6 {
        let router = fixture.router.clone();
        let token = fixture.token.clone();
        let body = serde_json::json!({
            "name": format!("race-{index}-{}", uuid::Uuid::new_v4()),
            "is_bot": false,
            "profile_username": username,
        });
        handles.push(tokio::spawn(async move {
            router
                .oneshot(request(
                    "POST",
                    "/api/v1/accounts",
                    &token,
                    None,
                    Body::from(body.to_string()),
                ))
                .await
                .unwrap()
                .status()
        }));
    }
    let mut created = 0;
    let mut conflicts = 0;
    for handle in handles {
        match handle.await.unwrap() {
            StatusCode::CREATED => created += 1,
            StatusCode::CONFLICT => conflicts += 1,
            other => panic!("unexpected status {other}"),
        }
    }
    assert_eq!((created, conflicts), (1, 5));
    assert_eq!(username_event_count(&fixture.module, &username).await, 1);
    assert_eq!(
        scalar_i64(
            &fixture.module,
            "SELECT count(*) FROM profiles WHERE username = $1",
            &username,
        )
        .await,
        1
    );
}
