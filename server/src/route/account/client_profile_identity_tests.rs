//! profile-identity-read-exposure: every `AccountResponse` route returns the Profile nanoid
//! (`profile_id`) and username (`profile_username`, null while unset) of the Account's Profile.
//! Real database and real use cases; only Keto is mocked.

use super::{
    count_rows, ensure_master_key_password_file, fixture, fixture_with_keto_write,
    keto_with_writes, request, Fixture,
};
use crate::handler::AppModule;
use axum::body::Body;
use axum::http::StatusCode;
use http_body_util::BodyExt;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
use kernel::interfaces::repository::{AggregateRepository, DependOnProfileRepository};
use kernel::prelude::entity::{AccountId, OrgRole, Profile, ProfileId};
use tower::ServiceExt;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Reply {
    status: StatusCode,
    raw: String,
    json: serde_json::Value,
}

async fn send(
    fixture: &Fixture,
    method: &str,
    uri: &str,
    token: &str,
    organization: Option<&str>,
    body: serde_json::Value,
) -> Reply {
    let response = fixture
        .router
        .clone()
        .oneshot(request(
            method,
            uri,
            token,
            organization,
            Body::from(body.to_string()),
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let raw = String::from_utf8_lossy(&bytes).to_string();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    Reply { status, raw, json }
}

async fn get(fixture: &Fixture, uri: &str, token: &str) -> Reply {
    send(fixture, "GET", uri, token, None, serde_json::Value::Null).await
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

async fn execute(module: &AppModule, sql: &str, bind: &str) {
    let mut executor = module.database_connection().connection().await.unwrap();
    let con: &mut sqlx::PgConnection = &mut executor;
    sqlx::query(sql).bind(bind).execute(con).await.unwrap();
}

fn unique_username() -> String {
    format!("u{}", uuid::Uuid::new_v4().simple())
}

/// Nanoid of the (single) Profile of the personal Account with this nanoid.
async fn profile_nanoid_of_account(module: &AppModule, account_nanoid: &str) -> String {
    scalar_string(
        module,
        "SELECT p.nanoid FROM profiles p JOIN accounts a ON a.id = p.account_id WHERE a.nanoid = $1",
        account_nanoid,
    )
    .await
    .expect("the account has a profile")
}

/// Creates a personal Account through the REST API and returns `(account_nanoid, response)`.
async fn create_account(
    fixture: &Fixture,
    token: &str,
    username: Option<&str>,
) -> (String, serde_json::Value) {
    let name = format!("identity-{}", uuid::Uuid::new_v4());
    let mut body = serde_json::json!({"name": name, "is_bot": false});
    if let Some(username) = username {
        body["profile_username"] = serde_json::json!(username);
    }
    let reply = send(fixture, "POST", "/api/v1/accounts", token, None, body).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.raw);
    (reply.json["id"].as_str().unwrap().to_string(), reply.json)
}

/// An Account whose Profile has no username, as every Profile created before the P4 cutover (the
/// API now requires a username): the username is removed from both the event stream and the row, so
/// the one-time setter still works on it. The returned body is the reply of the creation call with
/// `profile_username` shown as the stored state (null).
async fn create_account_without_username(
    fixture: &Fixture,
    token: &str,
) -> (String, serde_json::Value) {
    let (nanoid, mut created) = create_account(fixture, token, Some(&unique_username())).await;
    execute(
        &fixture.module,
        "UPDATE profile_events SET data = data - 'username' \
         WHERE id = (SELECT id FROM profiles WHERE account_id = (SELECT id FROM accounts WHERE nanoid = $1))",
        &nanoid,
    )
    .await;
    execute(
        &fixture.module,
        "UPDATE profiles SET username = NULL WHERE account_id = (SELECT id FROM accounts WHERE nanoid = $1)",
        &nanoid,
    )
    .await;
    created["profile_username"] = serde_json::Value::Null;
    (nanoid, created)
}

async fn owner_auth_id(module: &AppModule, account_nanoid: &str) -> String {
    let mut executor = module.database_connection().connection().await.unwrap();
    let con: &mut sqlx::PgConnection = &mut executor;
    sqlx::query_scalar::<_, i64>(
        "SELECT auth_id FROM auth_emumet_accounts WHERE emumet_id = (SELECT id FROM accounts WHERE nanoid = $1)",
    )
    .bind(account_nanoid)
    .fetch_one(con)
    .await
    .unwrap()
    .to_string()
}

/// Keto mock where only `allowed_subject` passes permission checks.
async fn mount_keto_allowing_only(keto: &MockServer, allowed_subject: &str) {
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

fn assert_identity(item: &serde_json::Value, profile_id: &str, username: Option<&str>) {
    assert_eq!(item["profile_id"].as_str(), Some(profile_id), "{item}");
    match username {
        Some(username) => assert_eq!(item["profile_username"].as_str(), Some(username), "{item}"),
        None => {
            assert!(
                item.get("profile_username").is_some_and(|v| v.is_null()),
                "profile_username must be present and null: {item}"
            );
        }
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn every_account_response_route_returns_the_profile_identity() {
    ensure_master_key_password_file();
    let keto = keto_with_writes().await;
    let fixture = fixture_with_keto_write(&keto.uri(), &keto.uri(), None).await;
    let token = fixture.fresh_token.clone();
    let username = unique_username();

    // POST /api/v1/accounts (with and without a username).
    let (with_name, created) = create_account(&fixture, &token, Some(&username)).await;
    let profile_with_name = profile_nanoid_of_account(&fixture.module, &with_name).await;
    assert_identity(&created, &profile_with_name, Some(&username));
    // Legacy fields are unchanged.
    assert_eq!(created["id"].as_str(), Some(with_name.as_str()));
    assert_eq!(created["is_bot"], false);
    assert!(created["moderation"].is_null());
    assert!(created["fields"].is_array());
    assert!(created["created_at"].is_string());
    let (without_name, created) = create_account_without_username(&fixture, &token).await;
    let profile_without_name = profile_nanoid_of_account(&fixture.module, &without_name).await;
    assert_identity(&created, &profile_without_name, None);

    // GET /api/v1/accounts/{id}.
    let reply = get(&fixture, &format!("/api/v1/accounts/{with_name}"), &token).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
    assert_identity(&reply.json, &profile_with_name, Some(&username));
    let reply = get(
        &fixture,
        &format!("/api/v1/accounts/{without_name}"),
        &token,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
    assert_identity(&reply.json, &profile_without_name, None);

    // GET /api/v1/accounts?ids=...
    let reply = get(
        &fixture,
        &format!("/api/v1/accounts?ids={with_name},{without_name}"),
        &token,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
    let items = reply.json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    for item in items {
        if item["id"].as_str() == Some(with_name.as_str()) {
            assert_identity(item, &profile_with_name, Some(&username));
        } else {
            assert_eq!(item["id"].as_str(), Some(without_name.as_str()));
            assert_identity(item, &profile_without_name, None);
        }
    }

    // GET /api/v1/accounts (cursor listing).
    let reply = get(&fixture, "/api/v1/accounts", &token).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
    let items = reply.json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    for item in items {
        if item["id"].as_str() == Some(with_name.as_str()) {
            assert_identity(item, &profile_with_name, Some(&username));
        } else {
            assert_identity(item, &profile_without_name, None);
        }
    }

    // PATCH /api/v1/accounts/{id} keeps the same identity.
    let reply = send(
        &fixture,
        "PATCH",
        &format!("/api/v1/accounts/{without_name}"),
        &token,
        None,
        serde_json::json!({"display_name": "Updated"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
    assert_eq!(reply.json["display_name"], "Updated");
    assert_identity(&reply.json, &profile_without_name, None);
    let reply = send(
        &fixture,
        "PATCH",
        &format!("/api/v1/accounts/{with_name}"),
        &token,
        None,
        serde_json::json!({"summary": "Updated summary"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
    assert_identity(&reply.json, &profile_with_name, Some(&username));

    // After the one-time username setter the value shows up on the next read.
    let later = unique_username();
    let reply = send(
        &fixture,
        "POST",
        &format!("/api/v1/profiles/{profile_without_name}/username"),
        &token,
        None,
        serde_json::json!({"username": later}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
    let reply = get(
        &fixture,
        &format!("/api/v1/accounts/{without_name}"),
        &token,
    )
    .await;
    assert_identity(&reply.json, &profile_without_name, Some(&later));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn another_users_profile_identity_is_never_exposed() {
    ensure_master_key_password_file();
    let keto = keto_with_writes().await;
    let fixture = fixture_with_keto_write(&keto.uri(), &keto.uri(), None).await;
    let username = unique_username();
    let (account, created) = create_account(&fixture, &fixture.fresh_token, Some(&username)).await;
    let profile = profile_nanoid_of_account(&fixture.module, &account).await;
    assert_identity(&created, &profile, Some(&username));
    let owner = owner_auth_id(&fixture.module, &account).await;
    keto.reset().await;
    mount_keto_allowing_only(&keto, &owner).await;

    // The owner still reads it.
    let reply = get(
        &fixture,
        &format!("/api/v1/accounts/{account}"),
        &fixture.fresh_token,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
    assert_identity(&reply.json, &profile, Some(&username));

    // Another authenticated user gets the same answers as before this change (not found / empty)
    // and no trace of the Profile identity.
    let reply = get(
        &fixture,
        &format!("/api/v1/accounts/{account}"),
        &fixture.token,
    )
    .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.raw);
    assert!(!reply.raw.contains(&profile) && !reply.raw.contains(&username));
    let reply = get(
        &fixture,
        &format!("/api/v1/accounts?ids={account}"),
        &fixture.token,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
    assert!(reply.json["items"].as_array().unwrap().is_empty());
    assert!(!reply.raw.contains(&profile) && !reply.raw.contains(&username));
    let reply = send(
        &fixture,
        "PATCH",
        &format!("/api/v1/accounts/{account}"),
        &fixture.token,
        None,
        serde_json::json!({"display_name": "Mallory"}),
    )
    .await;
    assert!(!reply.status.is_success(), "{}", reply.raw);
    assert!(!reply.raw.contains(&profile) && !reply.raw.contains(&username));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn deactivated_suspended_and_banned_accounts_keep_their_existing_visibility() {
    ensure_master_key_password_file();
    let keto = keto_with_writes().await;
    let fixture = fixture_with_keto_write(&keto.uri(), &keto.uri(), None).await;
    let token = fixture.fresh_token.clone();
    let (active, _) = create_account(&fixture, &token, Some(&unique_username())).await;
    let (deactivated, _) = create_account(&fixture, &token, Some(&unique_username())).await;
    let (suspended, _) = create_account(&fixture, &token, Some(&unique_username())).await;
    let (banned, _) = create_account_without_username(&fixture, &token).await;

    // Deactivation is projected asynchronously by the tailing projector, which these route tests do
    // not run; apply the projection state it produces (deleted_at) directly.
    execute(
        &fixture.module,
        "UPDATE accounts SET deleted_at = now() WHERE nanoid = $1",
        &deactivated,
    )
    .await;
    execute(
        &fixture.module,
        "UPDATE accounts SET suspended_at = now(), suspend_reason = 'reason' WHERE nanoid = $1",
        &suspended,
    )
    .await;
    execute(
        &fixture.module,
        "UPDATE accounts SET banned_at = now(), ban_reason = 'reason' WHERE nanoid = $1",
        &banned,
    )
    .await;
    let profile_active = profile_nanoid_of_account(&fixture.module, &active).await;
    let profile_suspended = profile_nanoid_of_account(&fixture.module, &suspended).await;
    let profile_banned = profile_nanoid_of_account(&fixture.module, &banned).await;
    let profile_deactivated = profile_nanoid_of_account(&fixture.module, &deactivated).await;

    // Listing: deactivated accounts are absent, suspended/banned ones are shown to their owner
    // with moderation (as before) and carry the Profile identity.
    let reply = get(&fixture, "/api/v1/accounts", &token).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
    let items = reply.json["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{}", reply.raw);
    assert!(!reply.raw.contains(&profile_deactivated));
    for item in items {
        match item["id"].as_str().unwrap() {
            id if id == active => {
                assert!(item["moderation"].is_null());
                assert_eq!(item["profile_id"].as_str(), Some(profile_active.as_str()));
            }
            id if id == suspended => {
                assert_eq!(item["moderation"]["type"], "suspended");
                assert_eq!(
                    item["profile_id"].as_str(),
                    Some(profile_suspended.as_str())
                );
                assert!(item["profile_username"].is_string());
            }
            id if id == banned => {
                assert_eq!(item["moderation"]["type"], "banned");
                assert_eq!(item["profile_id"].as_str(), Some(profile_banned.as_str()));
                assert!(item["profile_username"].is_null());
            }
            other => panic!("unexpected account {other}"),
        }
    }

    // ids and single reads exclude every non-active account exactly as before.
    let reply = get(
        &fixture,
        &format!("/api/v1/accounts?ids={active},{deactivated},{suspended},{banned}"),
        &token,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
    let items = reply.json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{}", reply.raw);
    assert_eq!(items[0]["id"].as_str(), Some(active.as_str()));
    assert_eq!(
        items[0]["profile_id"].as_str(),
        Some(profile_active.as_str())
    );
    for hidden in [&deactivated, &suspended, &banned] {
        let reply = get(&fixture, &format!("/api/v1/accounts/{hidden}"), &token).await;
        assert_eq!(
            reply.status,
            StatusCode::NOT_FOUND,
            "{hidden}: {}",
            reply.raw
        );
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn transferred_profile_identity_follows_the_new_owner() {
    ensure_master_key_password_file();
    let keto = keto_with_writes().await;
    let fixture = fixture_with_keto_write(&keto.uri(), &keto.uri(), Some(OrgRole::Member)).await;
    let organization = fixture.organization.nanoid().as_ref().to_string();
    let username = unique_username();
    let (personal, created) = create_account(&fixture, &fixture.fresh_token, Some(&username)).await;
    let profile = profile_nanoid_of_account(&fixture.module, &personal).await;
    assert_identity(&created, &profile, Some(&username));
    let display_name = created["display_name"].clone();

    // The events and projection update the real transfer use case commits.
    transfer_profile(&fixture, &profile, &organization).await;

    let reply = get(
        &fixture,
        &format!("/api/v1/accounts/{organization}"),
        &fixture.token,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
    assert_identity(&reply.json, &profile, Some(&username));
    assert_eq!(reply.json["display_name"], display_name);
    let reply = send(
        &fixture,
        "PATCH",
        &format!("/api/v1/accounts/{organization}"),
        &fixture.token,
        Some(&organization),
        serde_json::json!({"summary": "Now an organization"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
    assert_identity(&reply.json, &profile, Some(&username));

    // The former owner no longer has that Profile; the personal Account answers as it did before
    // this change for an Account without a Profile (not found), without any Profile identity.
    let reply = get(
        &fixture,
        &format!("/api/v1/accounts/{personal}"),
        &fixture.fresh_token,
    )
    .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.raw);
    assert!(!reply.raw.contains(&profile) && !reply.raw.contains(&username));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn organization_with_several_profiles_is_described_by_the_lowest_profile_id() {
    let keto = keto_with_writes().await;
    let fixture = fixture(&keto.uri(), Some(OrgRole::Member)).await;
    let organization = fixture.organization.nanoid().as_ref().to_string();
    let mut usernames = Vec::new();
    for display_name in ["First", "Second", "Third"] {
        let username = unique_username();
        let reply = send(
            &fixture,
            "POST",
            "/api/v1/profiles",
            &fixture.token,
            Some(&organization),
            serde_json::json!({"display_name": display_name, "username": username}),
        )
        .await;
        assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.raw);
        usernames.push(username);
    }
    let lowest = scalar_string(
        &fixture.module,
        "SELECT nanoid FROM profiles WHERE account_id = (SELECT id FROM accounts WHERE nanoid = $1) ORDER BY id LIMIT 1",
        &organization,
    )
    .await
    .unwrap();
    let lowest_display = scalar_string(
        &fixture.module,
        "SELECT display FROM profiles WHERE nanoid = $1",
        &lowest,
    )
    .await
    .unwrap();
    assert_eq!(
        count_rows(
            &fixture.module,
            "SELECT count(*) FROM profiles WHERE account_id = (SELECT id FROM accounts WHERE nanoid = $1)",
            &organization,
        )
        .await,
        3
    );

    // Repeated reads and the update response agree on the same Profile, and the displayed
    // fields come from it.
    for _ in 0..3 {
        let reply = get(
            &fixture,
            &format!("/api/v1/accounts/{organization}"),
            &fixture.token,
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
        assert_identity(&reply.json, &lowest, Some(&usernames[0]));
        assert_eq!(
            reply.json["display_name"].as_str(),
            Some(lowest_display.as_str())
        );
    }
    let reply = send(
        &fixture,
        "PATCH",
        &format!("/api/v1/accounts/{organization}"),
        &fixture.token,
        Some(&organization),
        serde_json::json!({"summary": "Org summary"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.raw);
    assert_identity(&reply.json, &lowest, Some(&usernames[0]));
    assert_eq!(
        reply.json["display_name"].as_str(),
        Some(lowest_display.as_str())
    );
    assert_eq!(reply.json["summary"], "Org summary");
    assert_eq!(
        scalar_string(
            &fixture.module,
            "SELECT summary FROM profiles WHERE nanoid = $1",
            &lowest,
        )
        .await
        .as_deref(),
        Some("Org summary"),
        "the update must be applied to the Profile that is returned"
    );
}

async fn transfer_profile(fixture: &Fixture, profile_nanoid: &str, organization_nanoid: &str) {
    let module = &fixture.module;
    let mut executor = module.database_connection().connection().await.unwrap();
    let (profile_id, from_id, to_id) = {
        let con: &mut sqlx::PgConnection = &mut executor;
        sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT p.id, p.account_id, (SELECT id FROM accounts WHERE nanoid = $2) \
             FROM profiles p WHERE p.nanoid = $1",
        )
        .bind(profile_nanoid)
        .bind(organization_nanoid)
        .fetch_one(con)
        .await
        .unwrap()
    };
    let profile_id = ProfileId::new(profile_id);
    let event = module
        .profile_repository()
        .save(
            &mut executor,
            Profile::transfer_account(
                profile_id.clone(),
                AccountId::new(from_id),
                AccountId::new(to_id),
            ),
        )
        .await
        .unwrap();
    let (profile, _) = module
        .profile_repository()
        .load(&mut executor, &profile_id)
        .await
        .unwrap()
        .into_parts();
    let _ = event;
    module
        .profile_read_model()
        .update(&mut executor, &profile)
        .await
        .unwrap();
}
