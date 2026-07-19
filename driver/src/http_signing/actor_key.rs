use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use kernel::interfaces::http_signing::ActorPublicKey;
use serde_json::Value;

/// Test-only: global cache of actor public keys injected via the test-mode API.
///
/// `fetch_actor_key` checks this before making an HTTP request. Keys are
/// inserted by the E2E test when the remote server (e.g. Iceshrimp) requires
/// authentication for its ActivityPub actor endpoint.
pub(super) static TEST_STATIC_ACTOR_KEYS: LazyLock<Mutex<HashMap<String, ActorPublicKey>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Insert an actor public key into the test global cache so that
/// `fetch_actor_key` returns it without making an HTTP request.
///
/// `owner` is the ActivityPub actor ID (URL) that owns this key. It is used
/// by the ActivityPub route handler to verify that the signing key belongs to
/// the actor who sent the activity. Pass an empty string if unknown.
#[cfg(any(test, feature = "test-mode"))]
pub fn inject_test_actor_key(key_id: &str, public_key_pem: String, owner: &str) {
    let mut map = TEST_STATIC_ACTOR_KEYS.lock().expect("poisoned lock");
    let pem_clone = public_key_pem.clone();
    let owner_val = if owner.is_empty() {
        // Derive owner from key_id by stripping the fragment.
        reqwest::Url::parse(key_id)
            .ok()
            .and_then(|mut url| {
                url.set_fragment(None);
                Some(url.to_string())
            })
            .unwrap_or_default()
    } else {
        owner.to_string()
    };
    map.insert(
        key_id.to_string(),
        ActorPublicKey {
            id: key_id.to_string(),
            owner: owner_val.clone(),
            public_key_pem,
        },
    );
    if let Ok(mut url) = reqwest::Url::parse(key_id) {
        url.set_fragment(None);
        let stripped = url.to_string();
        if stripped != key_id {
            map.insert(
                stripped,
                ActorPublicKey {
                    id: key_id.to_string(),
                    owner: owner_val,
                    public_key_pem: pem_clone,
                },
            );
        }
    }
}

pub(super) fn actor_public_key_from_json(
    key_id: &str,
    document: &Value,
) -> std::result::Result<ActorPublicKey, String> {
    if document.get("publicKeyPem").is_some() && key_value_matches_key_id(key_id, document) {
        return public_key_value_to_actor_key(key_id, document, document.get("owner"));
    }

    let public_key = document
        .get("publicKey")
        .ok_or_else(|| "publicKey field is missing".to_string())?;

    match public_key {
        Value::Object(_) if key_value_matches_key_id(key_id, public_key) => {
            public_key_value_to_actor_key(key_id, public_key, document.get("id"))
        }
        Value::Object(_) => Err("publicKey.id does not match keyId".to_string()),
        Value::Array(keys) => keys
            .iter()
            .find(|value| key_value_matches_key_id(key_id, value))
            .map(|value| public_key_value_to_actor_key(key_id, value, document.get("id")))
            .transpose()?
            .ok_or_else(|| "publicKey array does not contain a key matching keyId".to_string()),
        _ => Err("publicKey field is not an object".to_string()),
    }
}

fn key_value_matches_key_id(key_id: &str, value: &Value) -> bool {
    value
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| id == key_id)
}

fn public_key_value_to_actor_key(
    key_id: &str,
    value: &Value,
    owner_fallback: Option<&Value>,
) -> std::result::Result<ActorPublicKey, String> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(key_id)
        .to_string();
    let owner = value
        .get("owner")
        .or(owner_fallback)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let public_key_pem = value
        .get("publicKeyPem")
        .and_then(Value::as_str)
        .ok_or_else(|| "publicKeyPem field is missing".to_string())?
        .to_string();

    Ok(ActorPublicKey {
        id,
        owner,
        public_key_pem,
    })
}
