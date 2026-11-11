//! Profile username availability check through the REST API (real database, real use cases; only
//! Keto is mocked). The check is advisory, so every answer is compared with what the create /
//! set username APIs actually decide for the same value.

use super::{
    count_rows, ensure_master_key_password_file, fixture_with_keto_write, keto_with_writes,
    request, response_json, Fixture,
};
use crate::handler::AppModule;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::prelude::entity::RESERVED_PROFILE_USERNAMES;
use tower::ServiceExt;

const AVAILABILITY_PATH: &str = "/api/v1/profiles/username-availability";

fn unique_username() -> String {
    format!("u{}", uuid::Uuid::new_v4().simple())
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

async fn check(fixture: &Fixture, token: &str, username: &str) -> axum::response::Response {
    fixture
        .router
        .clone()
        .oneshot(request(
            "GET",
            &format!("{AVAILABILITY_PATH}?username={}", percent_encode(username)),
            token,
            None,
            Body::empty(),
        ))
        .await
        .unwrap()
}

/// Asserts the 200 answer and returns `(available, reason)`.
async fn answer(fixture: &Fixture, token: &str, username: &str) -> (bool, Option<String>) {
    let response = check(fixture, token, username).await;
    assert_eq!(response.status(), StatusCode::OK, "{username:?}");
    let json = response_json(response).await;
    assert_eq!(json["username"], username, "the checked value is echoed");
    let available = json["available"].as_bool().expect("available is a bool");
    let reason = json.get("reason").map(|reason| {
        reason
            .as_str()
            .expect("reason is a string when present")
            .to_string()
    });
    assert_eq!(
        available,
        reason.is_none(),
        "reason is present exactly when unavailable: {json}"
    );
    (available, reason)
}

async fn create_account_with_username(
    fixture: &Fixture,
    username: &str,
) -> axum::response::Response {
    let body = serde_json::json!({
        "name": format!("availability-{}", uuid::Uuid::new_v4()),
        "is_bot": false,
        "profile_username": username,
    });
    fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/accounts",
            &fixture.token,
            None,
            Body::from(body.to_string()),
        ))
        .await
        .unwrap()
}

/// Creates an Account without a username and returns `(account nanoid, profile nanoid)`.
async fn create_unnamed_account(fixture: &Fixture) -> (String, String) {
    let name = format!("availability-unnamed-{}", uuid::Uuid::new_v4());
    let response = fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/accounts",
            &fixture.token,
            None,
            Body::from(serde_json::json!({"name": name, "is_bot": false}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let account = response_json(response).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let profile = scalar_string(
        &fixture.module,
        "SELECT p.nanoid FROM profiles p JOIN accounts a ON a.id = p.account_id WHERE a.name = $1",
        &name,
    )
    .await
    .unwrap();
    (account, profile)
}

async fn set_username(
    fixture: &Fixture,
    profile_nanoid: &str,
    username: &str,
) -> axum::response::Response {
    fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/profiles/{profile_nanoid}/username"),
            &fixture.token,
            None,
            Body::from(serde_json::json!({ "username": username }).to_string()),
        ))
        .await
        .unwrap()
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

async fn username_event_count(module: &AppModule, username: &str) -> i64 {
    count_rows(
        module,
        "SELECT count(*) FROM profile_events WHERE data ->> 'username' = $1",
        username,
    )
    .await
}

async fn bootstrap() -> (wiremock::MockServer, Fixture) {
    ensure_master_key_password_file();
    let keto = keto_with_writes().await;
    let fixture = fixture_with_keto_write(&keto.uri(), &keto.uri(), None).await;
    (keto, fixture)
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn requires_authentication_and_the_username_parameter() {
    let (_keto, fixture) = bootstrap().await;

    let unauthenticated = fixture
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("{AVAILABILITY_PATH}?username=alice"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let invalid_token = check(&fixture, "not-a-jwt", "alice").await;
    assert_eq!(invalid_token.status(), StatusCode::UNAUTHORIZED);

    let missing = fixture
        .router
        .clone()
        .oneshot(request(
            "GET",
            AVAILABILITY_PATH,
            &fixture.token,
            None,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn free_malformed_and_reserved_usernames_match_what_create_and_set_decide() {
    let (_keto, fixture) = bootstrap().await;
    let token = fixture.fresh_token.clone();

    let free_for_create = unique_username();
    assert_eq!(
        answer(&fixture, &token, &free_for_create).await,
        (true, None)
    );
    let response = create_account_with_username(&fixture, &free_for_create).await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let (_, profile_nanoid) = create_unnamed_account(&fixture).await;
    let free_for_set = unique_username();
    assert_eq!(answer(&fixture, &token, &free_for_set).await, (true, None));
    let response = set_username(&fixture, &profile_nanoid, &free_for_set).await;
    assert_eq!(response.status(), StatusCode::OK);

    let boundary = "a".repeat(64);
    assert_eq!(
        answer(&fixture, &token, &boundary).await.1,
        None,
        "64 chars is valid"
    );
    let malformed = [
        "Alice".to_string(),
        " alice".to_string(),
        "alice ".to_string(),
        String::new(),
        "-alice".to_string(),
        ".alice".to_string(),
        "_alice".to_string(),
        "al@ce".to_string(),
        "ali/ce".to_string(),
        "alicé".to_string(),
        "ａlice".to_string(),
        "a\n".to_string(),
        "a".repeat(65),
    ];
    let (_, other_profile) = create_unnamed_account(&fixture).await;
    for value in &malformed {
        assert_eq!(
            answer(&fixture, &token, value).await,
            (false, Some("invalid_format".to_string())),
            "{value:?}"
        );
        let response = create_account_with_username(&fixture, value).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "create {value:?}"
        );
        let response = set_username(&fixture, &other_profile, value).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "set {value:?}");
    }

    assert_eq!(RESERVED_PROFILE_USERNAMES.len(), 17);
    for word in RESERVED_PROFILE_USERNAMES {
        assert_eq!(
            answer(&fixture, &token, word).await,
            (false, Some("reserved".to_string())),
            "{word}"
        );
        let response = create_account_with_username(&fixture, word).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "create {word}");
        let response = set_username(&fixture, &other_profile, word).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "set {word}");
    }
    assert_eq!(
        scalar_string(
            &fixture.module,
            "SELECT username FROM profiles WHERE nanoid = $1",
            &other_profile,
        )
        .await,
        None,
        "no rejected value may have been stored"
    );
    assert_eq!(
        answer(&fixture, &token, "admin1").await,
        (true, None),
        "a reserved word with a suffix is a different, valid name"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn taken_usernames_are_unavailable_from_both_assignment_paths_and_match_409() {
    let (_keto, fixture) = bootstrap().await;
    let token = fixture.fresh_token.clone();

    let via_create = unique_username();
    let response = create_account_with_username(&fixture, &via_create).await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let (_, profile_nanoid) = create_unnamed_account(&fixture).await;
    let via_set = unique_username();
    let response = set_username(&fixture, &profile_nanoid, &via_set).await;
    assert_eq!(response.status(), StatusCode::OK);

    let (_, victim_profile) = create_unnamed_account(&fixture).await;
    for taken in [&via_create, &via_set] {
        assert_eq!(
            answer(&fixture, &token, taken).await,
            (false, Some("taken".to_string())),
            "{taken}"
        );
        let response = create_account_with_username(&fixture, taken).await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "create {taken}");
        let response = set_username(&fixture, &victim_profile, taken).await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "set {taken}");
        assert_eq!(username_event_count(&fixture.module, taken).await, 1);
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_deleted_profiles_username_stays_taken_and_looks_the_same_as_one_in_use() {
    let (_keto, fixture) = bootstrap().await;
    let token = fixture.fresh_token.clone();

    let in_use = unique_username();
    let response = create_account_with_username(&fixture, &in_use).await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let deleted = unique_username();
    let response = create_account_with_username(&fixture, &deleted).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let account_nanoid = response_json(response).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let response = fixture
        .router
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/accounts/{account_nanoid}"),
            &fixture.token,
            None,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    // The tailing projector, which these route tests do not run, removes the Profile projection
    // row on Account deletion; apply that state directly. The events stay, as in production.
    {
        let mut executor = fixture
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        let con: &mut sqlx::PgConnection = &mut executor;
        sqlx::query("DELETE FROM profiles WHERE username = $1")
            .bind(&deleted)
            .execute(con)
            .await
            .unwrap();
    }
    assert_eq!(
        count_rows(
            &fixture.module,
            "SELECT count(*) FROM profiles WHERE username = $1",
            &deleted,
        )
        .await,
        0,
        "the Profile projection is gone"
    );
    assert_eq!(username_event_count(&fixture.module, &deleted).await, 1);

    let in_use_response = check(&fixture, &token, &in_use).await;
    let deleted_response = check(&fixture, &token, &deleted).await;
    assert_eq!(in_use_response.status(), StatusCode::OK);
    assert_eq!(deleted_response.status(), StatusCode::OK);
    let in_use_json = response_json(in_use_response).await;
    let deleted_json = response_json(deleted_response).await;
    assert_eq!(in_use_json["available"], false);
    assert_eq!(in_use_json["reason"], "taken");
    assert_eq!(deleted_json["available"], false);
    assert_eq!(deleted_json["reason"], "taken");
    assert_eq!(
        in_use_json.as_object().unwrap().keys().collect::<Vec<_>>(),
        deleted_json.as_object().unwrap().keys().collect::<Vec<_>>(),
        "in use and deleted are indistinguishable"
    );

    let response = create_account_with_username(&fixture, &deleted).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let (_, profile_nanoid) = create_unnamed_account(&fixture).await;
    let response = set_username(&fixture, &profile_nanoid, &deleted).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn checking_has_no_side_effects() {
    let (_keto, fixture) = bootstrap().await;
    let free = unique_username();

    assert_eq!(
        answer(&fixture, &fixture.fresh_token, &free).await,
        (true, None)
    );
    assert_eq!(
        answer(&fixture, &fixture.fresh_token, &free).await,
        (true, None),
        "checking does not reserve the username"
    );
    assert_eq!(username_event_count(&fixture.module, &free).await, 0);
    assert_eq!(
        count_rows(
            &fixture.module,
            "SELECT count(*) FROM profiles WHERE username = $1",
            &free,
        )
        .await,
        0
    );
    assert_eq!(
        count_rows(
            &fixture.module,
            "SELECT count(*) FROM auth_accounts WHERE client_id = $1",
            &fixture.fresh_subject,
        )
        .await,
        0,
        "a caller without an Account must not get an auth account created by a read"
    );

    let response = create_account_with_username(&fixture, &free).await;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "the username was still free after being checked"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_organization_header_does_not_make_the_check_create_an_auth_account() {
    let (_keto, fixture) = bootstrap().await;
    let free = unique_username();
    let existing_organization = fixture.organization.nanoid().as_ref().to_string();

    for organization in [existing_organization.as_str(), "no-such-organization"] {
        let response = fixture
            .router
            .clone()
            .oneshot(request(
                "GET",
                &format!("{AVAILABILITY_PATH}?username={free}"),
                &fixture.fresh_token,
                Some(organization),
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{organization}");
        let json = response_json(response).await;
        assert_eq!(json["available"], true);
        assert_eq!(
            count_rows(
                &fixture.module,
                "SELECT count(*) FROM auth_accounts WHERE client_id = $1",
                &fixture.fresh_subject,
            )
            .await,
            0,
            "{organization}: the organization header must not resolve (and create) an auth account"
        );
    }

    let unauthenticated = fixture
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("{AVAILABILITY_PATH}?username={free}"))
                .header("X-Organization-Id", &existing_organization)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        unauthenticated.status(),
        StatusCode::UNAUTHORIZED,
        "JWT authentication still applies with the header"
    );
    assert_eq!(
        count_rows(
            &fixture.module,
            "SELECT count(*) FROM auth_accounts WHERE client_id = $1",
            &fixture.fresh_subject,
        )
        .await,
        0
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn too_many_checks_are_limited_per_caller_with_retry_after() {
    let (_keto, fixture) = bootstrap().await;
    let free = unique_username();

    for attempt in 0..60 {
        let response = check(&fixture, &fixture.fresh_token, &free).await;
        assert_eq!(response.status(), StatusCode::OK, "attempt {attempt}");
    }
    let limited = check(&fixture, &fixture.fresh_token, &free).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after: u64 = limited
        .headers()
        .get("retry-after")
        .expect("Retry-After header")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry_after), "{retry_after}");

    let still_limited = check(&fixture, &fixture.fresh_token, "alice").await;
    assert_eq!(still_limited.status(), StatusCode::TOO_MANY_REQUESTS);

    let other_caller = check(&fixture, &fixture.token, &free).await;
    assert_eq!(
        other_caller.status(),
        StatusCode::OK,
        "another caller must not be affected"
    );
    assert_eq!(
        count_rows(
            &fixture.module,
            "SELECT count(*) FROM auth_accounts WHERE client_id = $1",
            &fixture.fresh_subject,
        )
        .await,
        0
    );
}
