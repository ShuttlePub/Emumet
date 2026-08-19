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

/// 2x2 red RGBA PNG.
const PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72, 0xb6, 0x0d,
    0x24, 0x00, 0x00, 0x00, 0x11, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x84, 0x19, 0x60, 0x0c, 0x00, 0x47, 0xca, 0x07, 0xf9, 0x67, 0x59, 0x6e, 0xb7, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

#[tokio::test]
#[ignore]
async fn media_upload_flows_to_actor_icon_end_to_end() {
    // Given: a clean service, an authenticated user, and an owned account.
    db::reset_test_data().await;
    let _server = EmumetServer::start().await;
    let jwt = auth::get_jwt_for_test_user().await;
    let client = reqwest::Client::new();

    let name = format!(
        "media-e2e-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let created = client
        .post("http://localhost:8080/api/v1/accounts")
        .bearer_auth(&jwt)
        .json(&serde_json::json!({"name": name, "is_bot": false}))
        .send()
        .await
        .expect("failed to create account");
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    let created: serde_json::Value = created.json().await.expect("invalid account response");
    let account_id = created["id"]
        .as_str()
        .expect("missing account id")
        .to_string();

    // When: an image is uploaded through the multipart media API.
    let file_part = reqwest::multipart::Part::bytes(PIXEL_PNG.to_vec())
        .file_name("pixel.png")
        .mime_str("image/png")
        .expect("invalid MIME type");
    let form = reqwest::multipart::Form::new().part("file", file_part);
    let uploaded = client
        .post("http://localhost:8080/api/v1/images")
        .bearer_auth(&jwt)
        .multipart(form)
        .send()
        .await
        .expect("image upload request failed");

    // Then: the image is registered with a public URL, content hash, and blurhash.
    assert_eq!(uploaded.status(), reqwest::StatusCode::CREATED);
    let uploaded: serde_json::Value = uploaded.json().await.expect("invalid upload response");
    let url = uploaded["url"]
        .as_str()
        .expect("missing image url")
        .to_string();
    assert!(url.starts_with("http://localhost:9000/emumet-media/images/"));
    assert!(!uploaded["hash"].as_str().unwrap_or_default().is_empty());
    assert!(!uploaded["blur_hash"]
        .as_str()
        .unwrap_or_default()
        .is_empty());

    // Then: the stored bytes are retrievable from the public URL.
    let fetched = client
        .get(&url)
        .send()
        .await
        .expect("failed to fetch stored image");
    assert_eq!(fetched.status(), reqwest::StatusCode::OK);
    assert_eq!(
        fetched.bytes().await.expect("invalid image body"),
        PIXEL_PNG
    );

    // When: the uploaded image is set as the account icon.
    let patched = client
        .patch(format!(
            "http://localhost:8080/api/v1/accounts/{account_id}"
        ))
        .bearer_auth(&jwt)
        .json(&serde_json::json!({"icon_url": url}))
        .send()
        .await
        .expect("icon patch failed");

    // Then: the patch succeeds and the Actor document exposes the uploaded icon.
    assert_eq!(patched.status(), reqwest::StatusCode::OK);
    let actor = client
        .get(format!("http://localhost:8080/ap/accounts/{account_id}"))
        .header("Accept", "application/activity+json")
        .send()
        .await
        .expect("actor fetch failed");
    assert_eq!(actor.status(), reqwest::StatusCode::OK);
    let actor: serde_json::Value = actor.json().await.expect("invalid actor response");
    assert_eq!(actor["icon"]["url"], url);
}
