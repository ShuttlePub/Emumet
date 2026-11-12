use super::ap_peer::{generate_cavage_signature, ApPeer, ReceivedActivity};
use super::auth;
use super::config::{ap_e2e_config, ApE2eConfig};
use super::db;
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

/// An Account of the E2E user together with its Profile (the ActivityPub actor).
pub struct TestAccount {
    /// Account nanoid (management API).
    pub id: String,
    pub name: String,
    /// Profile nanoid: the path component of the actor URL and of every `/api/v1/profiles/...` route.
    pub profile_id: String,
    /// Profile username (the acct name of the actor).
    pub username: String,
}

/// A Profile username that uses every character the format allows beyond letters and digits
/// (`.`, `_` and `-`), unique per call. Mastodon and Iceshrimp must federate with such names.
pub fn unique_username(prefix: &str) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let salt = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}.e2e_{millis}-{}", &salt[..6])
}

pub async fn setup_test_account_details() -> TestAccount {
    setup_test_account_with_username(&unique_username("ap")).await
}

pub async fn setup_test_account_with_username(username: &str) -> TestAccount {
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
        .post(format!("{}/api/v1/accounts", cfg.server_base_url))
        .bearer_auth(&jwt)
        .json(&serde_json::json!({
            "name": name,
            "is_bot": false,
            "profile_username": username,
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
    let id = account["id"]
        .as_str()
        .expect("account response missing id")
        .to_string();
    let profile_id = db::profile_nanoid_of_account(&id).await;
    TestAccount {
        id,
        name,
        profile_id,
        username: username.to_string(),
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

// ── URL builders ──────────────────────────────────────────────────

/// `{PUBLIC_BASE_URL}/ap/profiles/{profile_nanoid}`: the actor id.
pub fn profile_actor_url(cfg: &ApE2eConfig, profile_id: &str) -> String {
    format!(
        "{}/ap/profiles/{profile_id}",
        cfg.public_base_url.trim_end_matches('/')
    )
}

/// `(sign_inbox, send_inbox, target_actor)` for signed requests to the Profile inbox: the
/// signature covers the public URL while the request goes to the direct server URL.
pub fn profile_inbox_urls(cfg: &ApE2eConfig, profile_id: &str) -> (String, String, String) {
    (
        format!(
            "{}/ap/profiles/{profile_id}/inbox",
            cfg.public_base_url.trim_end_matches('/')
        ),
        format!(
            "{}/ap/profiles/{profile_id}/inbox",
            cfg.server_base_url.trim_end_matches('/')
        ),
        profile_actor_url(cfg, profile_id),
    )
}

// ── Profile relationship API ──────────────────────────────────────

fn profile_url(base_url: &str, profile_id: &str, tail: &str) -> String {
    format!(
        "{}/api/v1/profiles/{profile_id}{tail}",
        base_url.trim_end_matches('/')
    )
}

/// `POST /api/v1/profiles/{profile}/follows` (201 on success).
pub async fn post_follow(
    jwt: &str,
    profile_id: &str,
    base_url: &str,
    target: &str,
) -> reqwest::Response {
    e2e_http_client()
        .post(profile_url(base_url, profile_id, "/follows"))
        .bearer_auth(jwt)
        .json(&serde_json::json!({"target": target}))
        .send()
        .await
        .expect("follow request failed")
}

/// `DELETE /api/v1/profiles/{profile}/follows/{follow_id}` (204 on success).
pub async fn delete_follow(
    jwt: &str,
    profile_id: &str,
    base_url: &str,
    follow_id: &str,
) -> reqwest::Response {
    e2e_http_client()
        .delete(profile_url(
            base_url,
            profile_id,
            &format!("/follows/{follow_id}"),
        ))
        .bearer_auth(jwt)
        .send()
        .await
        .expect("unfollow request failed")
}

/// Follows the target and returns the follow id (asserts 201).
pub async fn follow_and_get_id(
    jwt: &str,
    profile_id: &str,
    base_url: &str,
    target: &str,
) -> String {
    let resp = post_follow(jwt, profile_id, base_url, target).await;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    assert_eq!(
        status,
        reqwest::StatusCode::CREATED,
        "follow failed: {body}"
    );
    let json: serde_json::Value = serde_json::from_str(&body).expect("follow response JSON");
    json["id"]
        .as_str()
        .expect("follow response missing id")
        .to_string()
}

pub async fn post_block(
    jwt: &str,
    profile_id: &str,
    base_url: &str,
    target: &str,
) -> reqwest::Response {
    e2e_http_client()
        .post(profile_url(base_url, profile_id, "/blocks"))
        .bearer_auth(jwt)
        .json(&serde_json::json!({"target": target}))
        .send()
        .await
        .expect("block request failed")
}

pub async fn delete_block(
    jwt: &str,
    profile_id: &str,
    base_url: &str,
    block_id: &str,
) -> reqwest::Response {
    e2e_http_client()
        .delete(profile_url(
            base_url,
            profile_id,
            &format!("/blocks/{block_id}"),
        ))
        .bearer_auth(jwt)
        .send()
        .await
        .expect("unblock request failed")
}

pub async fn get_blocks(jwt: &str, profile_id: &str, base_url: &str) -> serde_json::Value {
    get_list(jwt, profile_id, base_url, "/blocks").await
}

pub async fn post_mute(
    jwt: &str,
    profile_id: &str,
    base_url: &str,
    target: &str,
) -> reqwest::Response {
    e2e_http_client()
        .post(profile_url(base_url, profile_id, "/mutes"))
        .bearer_auth(jwt)
        .json(&serde_json::json!({"target": target}))
        .send()
        .await
        .expect("mute request failed")
}

pub async fn get_mutes(jwt: &str, profile_id: &str, base_url: &str) -> serde_json::Value {
    get_list(jwt, profile_id, base_url, "/mutes").await
}

/// `GET /api/v1/profiles/{profile}/following` or `/followers` (owner only).
pub async fn get_relations(
    jwt: &str,
    profile_id: &str,
    base_url: &str,
    list: &str,
) -> serde_json::Value {
    get_list(jwt, profile_id, base_url, &format!("/{list}")).await
}

async fn get_list(jwt: &str, profile_id: &str, base_url: &str, tail: &str) -> serde_json::Value {
    let resp = e2e_http_client()
        .get(profile_url(base_url, profile_id, tail))
        .bearer_auth(jwt)
        .send()
        .await
        .unwrap_or_else(|e| panic!("GET {tail} failed: {e}"));
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "GET {tail}");
    resp.json()
        .await
        .unwrap_or_else(|e| panic!("GET {tail} response not valid JSON: {e}"))
}

fn relation_count(list: &serde_json::Value) -> usize {
    list["items"].as_array().map(Vec::len).unwrap_or(0)
}

/// Poll `following` / `followers` of the Profile until it has at least `min_items` entries
/// (timeout after ~15 s).
pub async fn wait_for_relation_count(
    jwt: &str,
    profile_id: &str,
    base_url: &str,
    list: &str,
    min_items: usize,
) -> bool {
    for _ in 1..=30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if relation_count(&get_relations(jwt, profile_id, base_url, list).await) >= min_items {
            return true;
        }
    }
    false
}

/// Poll `following` / `followers` of the Profile until it has at most `max_items` entries.
pub async fn wait_for_relation_count_at_most(
    jwt: &str,
    profile_id: &str,
    base_url: &str,
    list: &str,
    max_items: usize,
) -> bool {
    for _ in 1..=30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if relation_count(&get_relations(jwt, profile_id, base_url, list).await) <= max_items {
            return true;
        }
    }
    false
}

/// Verifies the HTTP Signature of an activity received by the mock peer against `public_key_pem`
/// (the key published in the Profile's actor document) and returns the key id it was signed with.
pub async fn verify_activity_signature(
    activity: &ReceivedActivity,
    key_id: &str,
    public_key_pem: &str,
) -> kernel::interfaces::http_signing::SignatureVerificationResult {
    use kernel::interfaces::http_signing::{HttpSignatureVerificationInput, HttpSignatureVerifier};
    let mut verifier =
        driver::http_signing::HttpSignatureVerifierImpl::new().expect("signature verifier");
    verifier.cache_actor_key(key_id, public_key_pem.to_string());
    let headers = activity
        .headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect();
    verifier
        .verify(&HttpSignatureVerificationInput {
            method: "POST".to_string(),
            url: activity.url.clone(),
            headers,
            body: Some(activity.raw_body.clone()),
        })
        .await
        .expect("signature verification")
}

pub fn assert_signature_header(activity: &ReceivedActivity) {
    let found = activity.headers.iter().any(|(k, _)| {
        let kl = k.to_lowercase();
        kl == "signature" || kl == "authorization"
    });
    assert!(
        found,
        "Activity should have HTTP Signature (Signature or Authorization header)"
    );
}

/// The Profile actor document (public, ActivityPub media type).
pub async fn fetch_profile_actor(base_url: &str, profile_id: &str) -> serde_json::Value {
    let resp = e2e_http_client()
        .get(format!(
            "{}/ap/profiles/{profile_id}",
            base_url.trim_end_matches('/')
        ))
        .header(reqwest::header::ACCEPT, "application/activity+json")
        .send()
        .await
        .expect("actor request failed");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    resp.json().await.expect("actor response not valid JSON")
}

// ── Signed requests from a remote peer to the Profile inbox ───────

async fn post_signed_activity(
    peer: &ApPeer,
    sign_inbox: &str,
    send_inbox: &str,
    activity: serde_json::Value,
) -> reqwest::Response {
    let body_bytes = serde_json::to_vec(&activity).expect("serialize activity");
    let key_id = format!("{}#main-key", peer.actor_url);
    let signature_headers = generate_cavage_signature(
        "POST",
        sign_inbox,
        &body_bytes,
        &peer.private_key_pem,
        &key_id,
    )
    .await;
    let mut request = e2e_http_client().post(send_inbox);
    for (k, v) in &signature_headers {
        request = request.header(k.as_str(), v.as_str());
    }
    request
        .body(body_bytes)
        .send()
        .await
        .expect("signed inbox POST failed")
}

/// A signed Follow from `peer` to the Profile.
pub async fn post_signed_follow_direct(
    peer: &ApPeer,
    sign_inbox: &str,
    send_inbox: &str,
    target_actor: &str,
) -> reqwest::Response {
    let follow_id = format!("{}/activities/{}", peer.actor_url, uuid::Uuid::new_v4());
    post_signed_follow_with_id_direct(peer, sign_inbox, send_inbox, target_actor, &follow_id).await
}

pub async fn post_signed_follow_with_id_direct(
    peer: &ApPeer,
    sign_inbox: &str,
    send_inbox: &str,
    target_actor: &str,
    follow_id: &str,
) -> reqwest::Response {
    post_signed_activity(
        peer,
        sign_inbox,
        send_inbox,
        serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "type": "Follow",
            "actor": peer.actor_url,
            "object": target_actor,
            "id": follow_id,
        }),
    )
    .await
}

pub async fn post_signed_undo_follow_direct(
    peer: &ApPeer,
    sign_inbox: &str,
    send_inbox: &str,
    target_actor: &str,
    follow_id: &str,
) -> reqwest::Response {
    post_signed_activity(
        peer,
        sign_inbox,
        send_inbox,
        serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "type": "Undo",
            "actor": peer.actor_url,
            "id": format!("{follow_id}/undo"),
            "object": {
                "type": "Follow",
                "id": follow_id,
                "actor": peer.actor_url,
                "object": target_actor,
            },
        }),
    )
    .await
}

pub async fn post_signed_accept_direct(
    peer: &ApPeer,
    sign_inbox: &str,
    send_inbox: &str,
    follow_activity_id: &str,
    target_actor: &str,
) -> reqwest::Response {
    post_signed_activity(
        peer,
        sign_inbox,
        send_inbox,
        serde_json::json!({
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
        }),
    )
    .await
}

pub async fn post_signed_block_direct(
    peer: &ApPeer,
    sign_inbox: &str,
    send_inbox: &str,
    target_actor: &str,
) -> reqwest::Response {
    post_signed_activity(
        peer,
        sign_inbox,
        send_inbox,
        serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "type": "Block",
            "actor": peer.actor_url,
            "object": target_actor,
            "id": format!("{}/activities/block/{}", peer.actor_url, uuid::Uuid::new_v4()),
        }),
    )
    .await
}
