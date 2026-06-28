use super::ap_peer::{generate_cavage_signature, ApPeer, ReceivedActivity};
use super::auth;
use super::config::ap_e2e_config;
use super::server::EmumetServer;

/// Build an HTTP client suitable for E2E test requests to the Emumet server.
///
/// When `AP_TEST_ACCEPT_INVALID_CERTS=1` is set (as in the compose E2E runner),
/// the client accepts self-signed certificates.  Otherwise behaves like
/// `reqwest::Client::new()`.
pub fn e2e_http_client() -> reqwest::Client {
    let mut builder = reqwest::Client::builder();
    if std::env::var("AP_TEST_ACCEPT_INVALID_CERTS").as_deref() == Ok("1") {
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder.build().expect("failed to build E2E HTTP client")
}

pub struct TestAccount {
    pub id: String,
    pub name: String,
}

pub async fn setup_test_account_details() -> TestAccount {
    let cfg = ap_e2e_config();
    let jwt = auth::get_jwt_for_test_user().await;
    let client = e2e_http_client();
    let name = format!(
        "ap-e2e-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let resp = client
        .post(format!("{}/accounts", cfg.server_base_url))
        .bearer_auth(&jwt)
        .json(&serde_json::json!({"name": name, "is_bot": false}))
        .send()
        .await
        .expect("failed to create test account");
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    assert_eq!(
        status,
        reqwest::StatusCode::CREATED,
        "account creation failed: {body}"
    );
    let account: serde_json::Value =
        serde_json::from_str(&body).expect("failed to parse account response");
    TestAccount {
        id: account["id"]
            .as_str()
            .expect("account response missing id")
            .to_string(),
        name,
    }
}

pub async fn setup_test_account() -> String {
    setup_test_account_details().await.id
}

pub fn assert_content_type(resp: &reqwest::Response, expected: &str) {
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .expect("missing content-type");
    assert!(ct.contains(expected), "expected {expected}, got: {ct}");
}

pub async fn start_server_with_peer(peer: &ApPeer) -> EmumetServer {
    EmumetServer::start_with_ap_test(&peer.address().ip().to_string()).await
}

pub async fn post_follow(
    jwt: &str,
    account_nanoid: &str,
    base_url: &str,
    target: &str,
) -> reqwest::Response {
    e2e_http_client()
        .post(format!("{base_url}/accounts/{account_nanoid}/follow"))
        .bearer_auth(jwt)
        .json(&serde_json::json!({"target": target}))
        .send()
        .await
        .expect("follow request failed")
}

pub fn assert_signature_header(activity: &ReceivedActivity) {
    let found = activity.headers.iter().any(|(k, _)| {
        let kl = k.to_lowercase();
        kl == "signature" || kl == "authorization"
    });
    assert!(
        found,
        "Follow activity should have HTTP Signature (Signature or Authorization header)"
    );
}

pub async fn fetch_collection(
    base_url: &str,
    account_nanoid: &str,
    collection: &str,
) -> serde_json::Value {
    let resp = e2e_http_client()
        .get(format!("{base_url}/accounts/{account_nanoid}/{collection}"))
        .header(reqwest::header::ACCEPT, "application/activity+json")
        .send()
        .await
        .unwrap_or_else(|e| panic!("{collection} request failed: {e}"));
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    resp.json()
        .await
        .unwrap_or_else(|e| panic!("{collection} response not valid JSON: {e}"))
}

pub fn assert_collection_has_items(collection: &serde_json::Value, min: u64) {
    assert_eq!(collection["type"], "OrderedCollection");
    assert!(
        collection["totalItems"].as_u64().unwrap_or(0) >= min,
        "collection should have at least {min} items"
    );
}

pub async fn post_signed_follow(
    peer: &ApPeer,
    target_inbox: &str,
    target_actor: &str,
) -> reqwest::Response {
    let follow_activity = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Follow",
        "actor": peer.actor_url,
        "object": target_actor,
        "id": format!("{}/activities/{}", peer.actor_url, uuid::Uuid::new_v4()),
    });
    let body_bytes = serde_json::to_vec(&follow_activity).expect("serialize follow activity");
    let key_id = format!("{}#main-key", peer.actor_url);
    let signature_headers = generate_cavage_signature(
        "POST",
        target_inbox,
        &body_bytes,
        &peer.private_key_pem,
        &key_id,
    )
    .await;
    let mut request = e2e_http_client().post(target_inbox);
    for (k, v) in &signature_headers {
        request = request.header(k.as_str(), v.as_str());
    }
    request
        .body(body_bytes)
        .send()
        .await
        .expect("signed inbox POST failed")
}

pub async fn post_signed_accept(
    peer: &ApPeer,
    target_inbox: &str,
    follow_activity_id: &str,
    target_actor: &str,
) -> reqwest::Response {
    let accept_activity = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Accept",
        "actor": peer.actor_url,
        "object": {
            "type": "Follow",
            "id": follow_activity_id,
            "actor": target_actor,
            "object": peer.actor_url,
        },
        "id": format!("{}/activities/accept/{}", peer.actor_url, uuid::Uuid::new_v4()),
    });
    let body_bytes = serde_json::to_vec(&accept_activity).expect("serialize accept activity");
    let key_id = format!("{}#main-key", peer.actor_url);
    let signature_headers = generate_cavage_signature(
        "POST",
        target_inbox,
        &body_bytes,
        &peer.private_key_pem,
        &key_id,
    )
    .await;
    let mut request = e2e_http_client().post(target_inbox);
    for (k, v) in &signature_headers {
        request = request.header(k.as_str(), v.as_str());
    }
    request
        .body(body_bytes)
        .send()
        .await
        .expect("signed Accept POST failed")
}
