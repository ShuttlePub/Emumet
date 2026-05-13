mod support;

use std::time::Duration;

use support::auth;
use support::db;
use support::server::EmumetServer;

#[tokio::test]
#[ignore]
async fn login_create_account_and_verify_profile() {
    db::reset_test_data().await;

    let _server = EmumetServer::start().await;

    let jwt = auth::get_jwt_for_test_user().await;

    let client = reqwest::Client::new();
    let resp = client
        .post("http://localhost:8080/accounts")
        .bearer_auth(&jwt)
        .json(&serde_json::json!({"name": "E2E Test Account", "is_bot": false}))
        .send()
        .await
        .expect("failed to create account");

    let status = resp.status();
    let body = resp
        .text()
        .await
        .expect("failed to read account response body");
    assert_eq!(
        status,
        reqwest::StatusCode::CREATED,
        "account creation failed: {body}"
    );

    let account: serde_json::Value =
        serde_json::from_str(&body).expect("failed to parse account response");
    let account_id = account
        .get("id")
        .and_then(|v| v.as_str())
        .expect("account response missing id");

    let profiles = poll_profiles(&client, &jwt, account_id).await;

    assert_eq!(
        profiles.len(),
        1,
        "expected exactly one auto-created profile"
    );
    assert_eq!(
        profiles[0]
            .get("account_id")
            .and_then(|v| v.as_str())
            .expect("profile missing account_id"),
        account_id
    );
    assert_eq!(
        profiles[0]
            .get("display_name")
            .and_then(|v| v.as_str())
            .expect("profile missing display_name"),
        "E2E Test Account"
    );

    let update_url = format!("http://localhost:8080/accounts/{account_id}/profile");
    let resp1 = client
        .put(&update_url)
        .bearer_auth(&jwt)
        .json(&serde_json::json!({"display_name": "Updated Name 1"}))
        .send()
        .await
        .expect("first profile update failed");
    let status1 = resp1.status();
    let body1 = resp1.text().await.unwrap_or_default();
    assert_eq!(
        status1,
        reqwest::StatusCode::NO_CONTENT,
        "first update failed: {body1}"
    );

    let resp2 = client
        .put(&update_url)
        .bearer_auth(&jwt)
        .json(&serde_json::json!({"display_name": "Updated Name 2"}))
        .send()
        .await
        .expect("second profile update failed");
    let status2 = resp2.status();
    let body2 = resp2.text().await.unwrap_or_default();
    assert_eq!(
        status2,
        reqwest::StatusCode::NO_CONTENT,
        "second update failed (LWW regression - version mismatch?): {body2}"
    );

    assert_concurrent_account_updates_succeed(&client, &jwt, account_id).await;

    assert_metadata_lifecycle(&client, &jwt, account_id).await;
}

async fn assert_metadata_lifecycle(client: &reqwest::Client, jwt: &str, account_id: &str) {
    let create_url = format!("http://localhost:8080/accounts/{account_id}/metadata");
    let resp = client
        .post(&create_url)
        .bearer_auth(jwt)
        .json(&serde_json::json!({"label": "website", "content": "https://example.com"}))
        .send()
        .await
        .expect("failed to create metadata");
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    assert_eq!(
        status,
        reqwest::StatusCode::CREATED,
        "metadata create failed: {body}"
    );
    let created: serde_json::Value =
        serde_json::from_str(&body).expect("failed to parse metadata response");
    let metadata_nanoid = created
        .get("nanoid")
        .and_then(|v| v.as_str())
        .expect("metadata response missing nanoid")
        .to_string();

    let metadata = poll_metadata(client, jwt, account_id, true).await;
    assert_eq!(metadata.len(), 1, "expected single metadata after create");
    assert_eq!(
        metadata[0]
            .get("nanoid")
            .and_then(|v| v.as_str())
            .expect("metadata projection missing nanoid"),
        metadata_nanoid
    );

    let resource_url =
        format!("http://localhost:8080/accounts/{account_id}/metadata/{metadata_nanoid}");
    let resp = client
        .delete(&resource_url)
        .bearer_auth(jwt)
        .send()
        .await
        .expect("metadata delete failed");
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    assert_eq!(
        status,
        reqwest::StatusCode::NO_CONTENT,
        "metadata delete failed: {body}"
    );

    let metadata = poll_metadata(client, jwt, account_id, false).await;
    assert!(
        metadata.is_empty(),
        "metadata projection still visible after delete: {metadata:?}"
    );

    let resp = client
        .put(&resource_url)
        .bearer_auth(jwt)
        .json(&serde_json::json!({"label": "website", "content": "https://other.example.com"}))
        .send()
        .await
        .expect("metadata update-after-delete request failed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "expected 404 on update-after-delete (bug 2 regression)"
    );
}

async fn poll_metadata(
    client: &reqwest::Client,
    jwt: &str,
    account_id: &str,
    expect_present: bool,
) -> Vec<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let resp = client
            .get(format!(
                "http://localhost:8080/metadata?account_ids={account_id}"
            ))
            .bearer_auth(jwt)
            .send()
            .await
            .expect("failed to query metadata");
        let status = resp.status();
        let body = resp.text().await.expect("failed to read metadata response");
        assert_eq!(
            status,
            reqwest::StatusCode::OK,
            "metadata query failed (attempt {attempts}): {body}"
        );
        let metadata: Vec<serde_json::Value> =
            serde_json::from_str(&body).expect("failed to parse metadata response");
        let has = !metadata.is_empty();
        if has == expect_present {
            return metadata;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for metadata projection (expect_present={expect_present}) after {attempts} attempts"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

async fn assert_concurrent_account_updates_succeed(
    client: &reqwest::Client,
    jwt: &str,
    account_id: &str,
) {
    let account_update_url = format!("http://localhost:8080/accounts/{account_id}");
    for (n, expected_is_bot) in [(1u32, true), (2u32, false)] {
        let resp = client
            .put(&account_update_url)
            .bearer_auth(jwt)
            .json(&serde_json::json!({"is_bot": expected_is_bot}))
            .send()
            .await
            .expect("account update failed");
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        assert_eq!(
            status,
            reqwest::StatusCode::NO_CONTENT,
            "account update #{n} failed: {body}"
        );
    }
}

async fn poll_profiles(
    client: &reqwest::Client,
    jwt: &str,
    account_id: &str,
) -> Vec<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let resp = client
            .get(format!(
                "http://localhost:8080/profiles?account_ids={account_id}"
            ))
            .bearer_auth(jwt)
            .send()
            .await
            .expect("failed to query profiles");

        let status = resp.status();
        let body = resp.text().await.expect("failed to read profile response");
        assert_eq!(
            status,
            reqwest::StatusCode::OK,
            "profile query failed (attempt {attempts}): {body}"
        );

        let profiles: Vec<serde_json::Value> =
            serde_json::from_str(&body).expect("failed to parse profile response");
        if !profiles.is_empty() {
            return profiles;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for profile projection after {attempts} attempts"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}
