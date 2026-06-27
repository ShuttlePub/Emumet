use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::sleep;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

pub struct ApPeer {
    pub server: MockServer,
    pub base_url: String,
    pub actor_url: String,
    pub public_key_pem: String,
    pub private_key_pem: Vec<u8>,
    inbox_activities: Arc<Mutex<Vec<ReceivedActivity>>>,
}

#[derive(Debug, Clone)]
pub struct ReceivedActivity {
    pub body: serde_json::Value,
    pub headers: Vec<(String, String)>,
}

fn generate_rsa_keypair_bytes() -> (Vec<u8>, String) {
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use rsa::{RsaPrivateKey, RsaPublicKey};

    let mut rng = rand::thread_rng();
    let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("failed to generate RSA key");
    let public_key = RsaPublicKey::from(&private_key);

    let private_pem = private_key
        .to_pkcs8_pem(LineEnding::LF)
        .expect("failed to encode private key")
        .to_string()
        .into_bytes();
    let public_pem = public_key
        .to_public_key_pem(LineEnding::LF)
        .expect("failed to encode public key");

    (private_pem, public_pem)
}

impl ApPeer {
    pub async fn new(username: &str) -> Self {
        let (private_key_pem, public_key_pem) = generate_rsa_keypair_bytes();
        let server = MockServer::start().await;
        let port = server.address().port();
        let base_url = format!("http://127.0.0.1:{port}");
        let actor_url = format!("{base_url}/users/{username}");
        let inbox_activities: Arc<Mutex<Vec<ReceivedActivity>>> = Arc::new(Mutex::new(Vec::new()));

        let u = username.to_string();
        let a = actor_url.clone();
        let pk = public_key_pem.clone();

        let wf_a = a.clone();
        Mock::given(method("GET"))
            .and(path("/.well-known/webfinger"))
            .respond_with(move |req: &Request| {
                let resource = req
                    .url
                    .query_pairs()
                    .find(|(k, _)| k == "resource")
                    .map(|(_, v)| v.to_string())
                    .unwrap_or_default();
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/jrd+json")
                    .set_body_json(serde_json::json!({
                        "subject": resource,
                        "aliases": [wf_a],
                        "links": [{
                            "rel": "self",
                            "type": "application/activity+json",
                            "href": wf_a
                        }]
                    }))
            })
            .mount(&server)
            .await;

        let actor_a = a.clone();
        let actor_u = u.clone();
        let actor_pk = pk.clone();
        Mock::given(method("GET"))
            .and(path(format!("/users/{actor_u}")))
            .respond_with(move |_: &Request| {
                let actor_url = actor_a.clone();
                let key_id = format!("{actor_url}#main-key");
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/activity+json")
                    .set_body_json(serde_json::json!({
                        "@context": [
                            "https://www.w3.org/ns/activitystreams",
                            {"publicKey": "https://w3id.org/security/v1#publicKey"}
                        ],
                        "id": actor_url,
                        "type": "Person",
                        "preferredUsername": actor_u,
                        "name": format!("Mock {}", actor_u),
                        "url": actor_url.clone(),
                        "inbox": format!("{}/inbox", actor_url),
                        "outbox": format!("{}/outbox", actor_url),
                        "followers": format!("{}/followers", actor_url),
                        "following": format!("{}/following", actor_url),
                        "publicKey": {
                            "id": key_id,
                            "owner": actor_url,
                            "publicKeyPem": actor_pk
                        }
                    }))
            })
            .mount(&server)
            .await;

        let activities = inbox_activities.clone();
        let inbox_u = u.clone();
        Mock::given(method("POST"))
            .and(path(format!("/users/{inbox_u}/inbox")))
            .respond_with(move |req: &Request| {
                let act = ReceivedActivity {
                    body: req.body_json().unwrap_or(serde_json::Value::Null),
                    headers: req
                        .headers
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                        .collect(),
                };
                activities.lock().unwrap().push(act);
                ResponseTemplate::new(202)
            })
            .mount(&server)
            .await;

        Self {
            server,
            base_url,
            actor_url,
            public_key_pem,
            private_key_pem,
            inbox_activities,
        }
    }

    pub fn received_activities(&self) -> Vec<ReceivedActivity> {
        self.inbox_activities.lock().unwrap().clone()
    }

    pub fn clear_inbox(&self) {
        self.inbox_activities.lock().unwrap().clear();
    }

    pub fn address(&self) -> SocketAddr {
        *self.server.address()
    }
}

/// Generate Cavage HTTP Signature headers using the driver's HttpSignerImpl.
pub async fn generate_cavage_signature(
    method: &str,
    url: &str,
    body: &[u8],
    private_key_pem: &[u8],
    key_id: &str,
) -> Vec<(String, String)> {
    use std::collections::HashMap;

    use driver::http_signing::HttpSignerImpl;
    use kernel::interfaces::crypto::SigningAlgorithm;
    use kernel::interfaces::http_signing::{HttpSigner, HttpSigningRequest};

    let parsed_url = url::Url::parse(url).expect("invalid URL");
    let host = parsed_url.host_str().unwrap_or("localhost");
    let host_header = match parsed_url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    };
    let date = httpdate::fmt_http_date(std::time::SystemTime::now());

    let digest = {
        use sha2::Digest;
        let hash = sha2::Sha256::digest(body);
        format!(
            "SHA-256={}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, hash)
        )
    };

    let mut headers = HashMap::new();
    headers.insert("host".to_string(), host_header.clone());
    headers.insert("date".to_string(), date.clone());
    headers.insert("digest".to_string(), digest.clone());
    headers.insert(
        "content-type".to_string(),
        "application/activity+json".to_string(),
    );

    let request = HttpSigningRequest {
        method: method.to_string(),
        url: url.to_string(),
        headers,
        body: Some(body.to_vec()),
    };

    let signer = HttpSignerImpl;
    let result = signer
        .sign(
            &request,
            private_key_pem,
            key_id,
            &SigningAlgorithm::Rsa2048,
        )
        .await
        .expect("signing failed");

    // Return both the Cavage headers and the basic headers
    let mut result_headers: Vec<(String, String)> = result
        .cavage_headers
        .into_iter()
        .map(|(k, v)| (k, v))
        .collect();
    result_headers.push(("host".to_string(), host_header));
    result_headers.push(("date".to_string(), date));
    result_headers.push(("digest".to_string(), digest));
    result_headers.push((
        "content-type".to_string(),
        "application/activity+json".to_string(),
    ));
    result_headers
}

/// Polls the mock peer's inbox for an activity with the given `type` field,
/// retrying every 500ms up to `timeout`.  Returns `None` if not received.
pub async fn wait_for_activity(
    peer: &ApPeer,
    activity_type: &str,
    timeout: Duration,
) -> Option<ReceivedActivity> {
    let start = std::time::Instant::now();
    loop {
        let activities = peer.received_activities();
        if let Some(act) = activities
            .into_iter()
            .find(|a| a.body.get("type").and_then(|v| v.as_str()) == Some(activity_type))
        {
            return Some(act);
        }
        if start.elapsed() >= timeout {
            return None;
        }
        sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn ap_peer_serves_actor_document() {
        let peer = ApPeer::new("testuser").await;
        let client = reqwest::Client::new();

        let resp = client
            .get(&format!("{}/users/testuser", peer.base_url))
            .header("accept", "application/activity+json")
            .send()
            .await
            .expect("failed to fetch actor");

        assert_eq!(resp.status(), 200);
        let actor: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(actor["type"], "Person");
        assert_eq!(actor["preferredUsername"], "testuser");
        assert!(actor["publicKey"]["publicKeyPem"]
            .as_str()
            .unwrap()
            .contains("BEGIN PUBLIC KEY"));
    }

    #[tokio::test]
    #[ignore]
    async fn ap_peer_serves_webfinger() {
        let peer = ApPeer::new("alice").await;
        let client = reqwest::Client::new();

        let resp = client
            .get(&format!(
                "{}/.well-known/webfinger?resource=acct:alice@localhost",
                peer.base_url
            ))
            .send()
            .await
            .expect("failed to fetch webfinger");

        assert_eq!(resp.status(), 200);
        let wf: serde_json::Value = resp.json().await.unwrap();
        assert!(wf["subject"].as_str().unwrap().contains("alice"));
        let links = wf["links"].as_array().unwrap();
        assert!(links.iter().any(|l| l["rel"] == "self"));
    }

    #[tokio::test]
    #[ignore]
    async fn ap_peer_captures_inbox_activity() {
        let peer = ApPeer::new("bob").await;
        let client = reqwest::Client::new();

        let activity = serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "type": "Follow",
            "actor": "https://remote.example.com/users/alice",
            "object": format!("{}/users/bob", peer.base_url)
        });

        let resp = client
            .post(&format!("{}/users/bob/inbox", peer.base_url))
            .header("content-type", "application/activity+json")
            .json(&activity)
            .send()
            .await
            .expect("failed to post to inbox");

        assert_eq!(resp.status(), 202);
        let received = peer.received_activities();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].body["type"], "Follow");
    }
}
