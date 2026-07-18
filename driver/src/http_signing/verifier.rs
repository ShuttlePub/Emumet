use std::collections::HashMap;
use std::time::Duration;

use error_stack::{Report, Result};
use kernel::interfaces::http_signing::{
    ActorPublicKey, HttpSignatureVerificationInput, HttpSignatureVerifier,
    SignatureVerificationResult,
};
use kernel::KernelError;

use super::actor_key::{actor_public_key_from_json, TEST_STATIC_ACTOR_KEYS};
use super::cavage::parser::parse_cavage_signature;
use super::cavage::{
    cavage_signing_string, get_header, validate_date_header, validate_digest_header,
    validate_required_signed_headers, verify_cavage_signature,
};
use super::fetch::fetch_limited_json;

#[derive(Debug, Clone)]
pub struct HttpSignatureVerifierImpl {
    client: reqwest::Client,
    max_response_bytes: usize,
    date_tolerance: Duration,
    pub(super) static_actor_keys: HashMap<String, ActorPublicKey>,
}

impl HttpSignatureVerifierImpl {
    pub fn new() -> Result<Self, KernelError> {
        let client_builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10));
        #[cfg(any(test, feature = "test-mode"))]
        let client_builder = if std::env::var("AP_TEST_ACCEPT_INVALID_CERTS").as_deref() == Ok("1")
        {
            client_builder.danger_accept_invalid_certs(true)
        } else {
            client_builder
        };
        let client = client_builder.build().map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to build HTTP client: {e}"))
        })?;

        Ok(Self {
            client,
            max_response_bytes: 1024 * 1024,
            date_tolerance: Duration::from_secs(5 * 60),
            static_actor_keys: HashMap::new(),
        })
    }

    /// Register an actor key that will be returned from cache without an HTTP fetch.
    ///
    /// This is intended for test environments where the remote ActivityPub server
    /// requires authentication for its actor endpoint. The calling test retrieves
    /// the public key via an authenticated API and injects it here, allowing
    /// HTTP Signature verification to succeed without a direct key fetch.
    ///
    /// `key_id` must match the full keyId URI (including the `#main-key` fragment
    /// if present in the HTTP Signature). The method normalizes by checking both
    /// the raw key and the fragment-stripped URL.
    #[cfg(any(test, feature = "test-mode"))]
    pub fn cache_actor_key(&mut self, key_id: &str, public_key_pem: String) {
        let key = ActorPublicKey {
            id: key_id.to_string(),
            owner: String::new(),
            public_key_pem,
        };
        self.static_actor_keys
            .insert(key_id.to_string(), key.clone());

        // Also index by fragment-stripped URL so fetch_actor_key can hit
        // the cache even when the caller strips the fragment first.
        if let Ok(mut url) = reqwest::Url::parse(key_id) {
            url.set_fragment(None);
            if url.as_str() != key_id {
                self.static_actor_keys.insert(url.to_string(), key);
            }
        }
    }
}

impl Default for HttpSignatureVerifierImpl {
    fn default() -> Self {
        Self::new().expect("HTTP signature verifier client construction should not fail")
    }
}

impl HttpSignatureVerifier for HttpSignatureVerifierImpl {
    async fn verify(
        &self,
        request: &HttpSignatureVerificationInput,
    ) -> Result<SignatureVerificationResult, KernelError> {
        if get_header(&request.headers, "signature-input").is_some() {
            return Ok(SignatureVerificationResult::Invalid(
                "RFC 9421 verification is not implemented".to_string(),
            ));
        }

        let signature_header = match get_header(&request.headers, "signature") {
            Some(value) => value,
            None => {
                return Ok(SignatureVerificationResult::Invalid(
                    "MissingSignature: Signature header is required".to_string(),
                ));
            }
        };

        let parsed = match parse_cavage_signature(signature_header) {
            Ok(parsed) => parsed,
            Err(message) => {
                return Ok(SignatureVerificationResult::Invalid(format!(
                    "MalformedSignature: {message}"
                )));
            }
        };

        if let Err(message) = validate_required_signed_headers(request, &parsed.headers) {
            return Ok(SignatureVerificationResult::Invalid(message));
        }

        if let Err(message) = validate_date_header(request, self.date_tolerance) {
            return Ok(SignatureVerificationResult::Invalid(message));
        }

        if let Err(message) = validate_digest_header(request) {
            return Ok(SignatureVerificationResult::Invalid(message));
        }

        let signing_string = match cavage_signing_string(request, &parsed.headers) {
            Ok(value) => value,
            Err(message) => return Ok(SignatureVerificationResult::Invalid(message)),
        };

        let actor_key = match self.fetch_actor_key(&parsed.key_id).await {
            Ok(value) => value,
            Err(e) => {
                return Ok(SignatureVerificationResult::Invalid(format!(
                    "KeyFetchFailed: {e:?}"
                )));
            }
        };

        let valid = verify_cavage_signature(
            &signing_string,
            &parsed.signature,
            actor_key.public_key_pem.as_bytes(),
            &parsed.algorithm,
        );

        match valid {
            Ok(true) => Ok(SignatureVerificationResult::Valid {
                key_id: parsed.key_id,
            }),
            Ok(false) => Ok(SignatureVerificationResult::Invalid(
                "InvalidSignature: signature does not match".to_string(),
            )),
            Err(message) => Ok(SignatureVerificationResult::Invalid(message)),
        }
    }

    async fn fetch_actor_key(&self, key_id: &str) -> Result<ActorPublicKey, KernelError> {
        // Check test-mode global cache first
        if let Ok(map) = TEST_STATIC_ACTOR_KEYS.lock() {
            if let Some(key) = map.get(key_id) {
                return Ok(key.clone());
            }
            // Also try fragment-stripped URL
            if let Ok(mut url) = reqwest::Url::parse(key_id) {
                url.set_fragment(None);
                let stripped = url.to_string();
                if stripped != key_id {
                    if let Some(key) = map.get(&stripped) {
                        return Ok(key.clone());
                    }
                }
            }
        }

        if let Some(key) = self.static_actor_keys.get(key_id) {
            return Ok(key.clone());
        }

        let mut url = reqwest::Url::parse(key_id).map_err(|e| {
            Report::new(KernelError::Rejected)
                .attach_printable(format!("MalformedSignature: invalid keyId URL: {e}"))
        })?;
        url.set_fragment(None);

        if let Some(key) = self.static_actor_keys.get(url.as_str()) {
            return Ok(key.clone());
        }

        let body = fetch_limited_json(&self.client, self.max_response_bytes, url).await?;
        actor_public_key_from_json(key_id, &body).map_err(|message| {
            Report::new(KernelError::Rejected).attach_printable(format!(
                "KeyFetchFailed: actor document does not contain a usable public key: {message}"
            ))
        })
    }
}
