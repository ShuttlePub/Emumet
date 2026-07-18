#[allow(dead_code)]
mod support;

use support::auth;
use support::db;
use support::server::EmumetServer;

#[tokio::test]
#[ignore]
async fn login_create_and_update_integrated_account() {
    // Given: a clean service and an authenticated user.
    db::reset_test_data().await;
    let _server = EmumetServer::start().await;
    let jwt = auth::get_jwt_for_test_user().await;
    let client = reqwest::Client::new();

    // When: an account is created.
    let created = client
        .post("http://localhost:8080/api/v1/accounts")
        .bearer_auth(&jwt)
        .json(&serde_json::json!({"name": "E2E Test Account", "is_bot": false}))
        .send()
        .await
        .expect("failed to create account");

    // Then: the response is the integrated resource without public_key.
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    let created: serde_json::Value = created.json().await.expect("invalid account response");
    let account_id = created["id"].as_str().expect("missing account id");
    assert_eq!(created["display_name"], "E2E Test Account");
    assert_eq!(created["fields"], serde_json::json!([]));
    assert!(created.get("public_key").is_none());

    // When: profile attributes, is_bot, and fields are patched together.
    let account_url = format!("http://localhost:8080/api/v1/accounts/{account_id}");
    let updated = client
        .patch(&account_url)
        .bearer_auth(&jwt)
        .json(&serde_json::json!({
            "display_name": "Updated Name",
            "summary": "Integrated account",
            "is_bot": true,
            "fields": [
                {"label": "Website", "content": "https://example.com"},
                {"label": "GitHub", "content": "https://github.com/example"}
            ]
        }))
        .send()
        .await
        .expect("integrated patch failed");

    // Then: the updated integrated resource is returned immediately.
    assert_eq!(updated.status(), reqwest::StatusCode::OK);
    let updated: serde_json::Value = updated.json().await.expect("invalid patch response");
    assert_eq!(updated["display_name"], "Updated Name");
    assert_eq!(updated["summary"], "Integrated account");
    assert_eq!(updated["is_bot"], true);
    assert_eq!(updated["fields"].as_array().map(Vec::len), Some(2));

    // When: a second patch clears summary and fully replaces fields by index.
    let replaced = client
        .patch(&account_url)
        .bearer_auth(&jwt)
        .json(&serde_json::json!({
            "summary": null,
            "fields": [{"label": "Website", "content": "https://other.example"}]
        }))
        .send()
        .await
        .expect("replacement patch failed");

    // Then: null clears, absent keys remain unchanged, and leftover fields are deleted.
    assert_eq!(replaced.status(), reqwest::StatusCode::OK);
    let replaced: serde_json::Value = replaced.json().await.expect("invalid patch response");
    assert!(
        replaced["summary"].is_null(),
        "summary was not cleared: {replaced}"
    );
    assert_eq!(replaced["display_name"], "Updated Name");
    assert_eq!(
        replaced["fields"],
        serde_json::json!([{"label": "Website", "content": "https://other.example"}])
    );

    // When: the resource is fetched through both single and list endpoints.
    let single: serde_json::Value = client
        .get(&account_url)
        .bearer_auth(&jwt)
        .send()
        .await
        .expect("single account request failed")
        .json()
        .await
        .expect("invalid single account response");
    let list: serde_json::Value = client
        .get(format!(
            "http://localhost:8080/api/v1/accounts?ids={account_id}"
        ))
        .bearer_auth(&jwt)
        .send()
        .await
        .expect("account list request failed")
        .json()
        .await
        .expect("invalid account list response");

    // Then: both surfaces expose the same integrated shape and cursor envelope.
    assert_eq!(single["fields"], replaced["fields"]);
    assert_eq!(list["items"][0]["id"], account_id);
    assert_eq!(list["first"], account_id);
    assert_eq!(list["last"], account_id);
}
