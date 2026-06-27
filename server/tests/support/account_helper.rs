use super::ap_peer::{generate_cavage_signature, ApPeer, ReceivedActivity};
use super::auth;
use super::config::ap_e2e_config;
use super::server::EmumetServer;

pub async fn setup_test_account() -> String {
    let cfg = ap_e2e_config();
    let jwt = auth::get_jwt_for_test_user().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/accounts", cfg.server_base_url))
        .bearer_auth(&jwt)
        .json(&serde_json::json!({
            "name": format!("ap-e2e-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()),
            "is_bot": false,
        }))
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
    account["id"]
        .as_str()
        .expect("account response missing id")
        .to_string()
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
    let allowed_host = format!("127.0.0.1:{}", peer.address().port());
    EmumetServer::start_with_ap_test(&allowed_host).await
}

pub async fn post_follow(
    jwt: &str,
    account_nanoid: &str,
    base_url: &str,
    target: &str,
) -> reqwest::Response {
    reqwest::Client::new()
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
    let resp = reqwest::Client::new()
        .get(format!("{base_url}/accounts/{account_nanoid}/{collection}"))
        .header(reqwest::header::ACCEPT, "application/activity+json")
        .send()
        .await
        .expect("{collection} request failed");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    resp.json()
        .await
        .expect("{collection} response not valid JSON")
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
    );
    let mut request = reqwest::Client::new().post(target_inbox);
    for (k, v) in &signature_headers {
        request = request.header(k.as_str(), v.as_str());
    }
    request
        .body(body_bytes)
        .send()
        .await
        .expect("signed inbox POST failed")
}
