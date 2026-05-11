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
