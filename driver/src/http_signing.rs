use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, SystemTime};

use base64::{engine::general_purpose, Engine as _};
use bytes::Bytes;
use error_stack::{Report, Result};
use http_body_util::Full;
use kernel::interfaces::crypto::SignatureVerifier as CryptoSignatureVerifier;
use kernel::interfaces::http_signing::{
    ActorPublicKey, HttpSignatureVerificationInput, HttpSignatureVerifier, HttpSigner,
    HttpSigningRequest, HttpSigningResponse, SignatureVerificationResult,
};
use kernel::KernelError;
use reqwest::header::{ACCEPT, LOCATION, USER_AGENT};
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::sha2::Sha256;
use rsa::signature::{SignatureEncoding, Signer as RsaSigner};
use rsa::RsaPrivateKey;
use serde_json::Value;
use sha2::Digest;

struct RsaCavageSignerKey {
    private_key: RsaPrivateKey,
    key_id: String,
}

impl RsaCavageSignerKey {
    fn new(private_key_pem: &[u8], key_id: &str) -> Result<Self, KernelError> {
        let pem_str = std::str::from_utf8(private_key_pem).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Invalid UTF-8 in private key PEM: {e}"))
        })?;
        let private_key = RsaPrivateKey::from_pkcs8_pem(pem_str).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to parse private key PEM: {e}"))
        })?;
        Ok(Self {
            private_key,
            key_id: key_id.to_string(),
        })
    }
}

impl http_msgsign_draft::sign::SignerKey for RsaCavageSignerKey {
    fn id(&self) -> String {
        self.key_id.clone()
    }

    fn algorithm(&self) -> String {
        "rsa-sha256".to_string()
    }

    fn sign(&self, target: &[u8]) -> Vec<u8> {
        let signing_key = SigningKey::<Sha256>::new(self.private_key.clone());
        signing_key.sign(target).to_bytes().to_vec()
    }
}

struct RsaRfc9421SignerKey {
    private_key: RsaPrivateKey,
    key_id: String,
}

impl RsaRfc9421SignerKey {
    fn new(private_key_pem: &[u8], key_id: &str) -> Result<Self, KernelError> {
        let pem_str = std::str::from_utf8(private_key_pem).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Invalid UTF-8 in private key PEM: {e}"))
        })?;
        let private_key = RsaPrivateKey::from_pkcs8_pem(pem_str).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to parse private key PEM: {e}"))
        })?;
        Ok(Self {
            private_key,
            key_id: key_id.to_string(),
        })
    }
}

impl http_msgsign::SignerKey for RsaRfc9421SignerKey {
    const ALGORITHM: &'static str = "rsa-v1_5-sha256";

    fn key_id(&self) -> String {
        self.key_id.clone()
    }

    fn sign(&self, target: &[u8]) -> Vec<u8> {
        let signing_key = SigningKey::<Sha256>::new(self.private_key.clone());
        signing_key.sign(target).to_bytes().to_vec()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HttpSignerImpl;

impl HttpSigner for HttpSignerImpl {
    async fn sign(
        &self,
        request: &HttpSigningRequest,
        private_key_pem: &[u8],
        key_id: &str,
        _algorithm: &kernel::interfaces::crypto::SigningAlgorithm,
    ) -> Result<HttpSigningResponse, KernelError> {
        use http_msgsign::RequestSign as Rfc9421RequestSign;
        use http_msgsign_draft::sign::RequestSign as CavageRequestSign;

        let cavage_req = build_http_request(request)?;
        let rfc9421_req = build_http_request(request)?;

        let cavage_key = RsaCavageSignerKey::new(private_key_pem, key_id)?;
        let mut cavage_params_builder = http_msgsign_draft::sign::SignatureParams::builder()
            .add_request_target()
            .add_header("host")
            .add_header("date");
        if get_header(&request.headers, "digest").is_some() {
            cavage_params_builder = cavage_params_builder.add_header("digest");
        }
        let cavage_params = cavage_params_builder.build().map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to build Cavage signature params: {e}"))
        })?;

        let signed_cavage = CavageRequestSign::sign(cavage_req, &cavage_key, &cavage_params)
            .await
            .map_err(|e| {
                Report::new(KernelError::Internal)
                    .attach_printable(format!("Cavage signing failed: {e}"))
            })?;

        let cavage_headers = extract_headers(&signed_cavage);

        let rfc9421_key = RsaRfc9421SignerKey::new(private_key_pem, key_id)?;
        let rfc9421_params = http_msgsign::SignatureParams::builder()
            .add_derive(
                http_msgsign::components::Derive::Method,
                http_msgsign::components::params::FieldParameter::default(),
            )
            .add_derive(
                http_msgsign::components::Derive::TargetUri,
                http_msgsign::components::params::FieldParameter::default(),
            )
            .add_derive(
                http_msgsign::components::Derive::Authority,
                http_msgsign::components::params::FieldParameter::default(),
            )
            .gen_created()
            .build()
            .map_err(|e| {
                Report::new(KernelError::Internal)
                    .attach_printable(format!("Failed to build RFC 9421 signature params: {e}"))
            })?;

        let signed_rfc9421 =
            Rfc9421RequestSign::sign(rfc9421_req, &rfc9421_key, "sig1", &rfc9421_params)
                .await
                .map_err(|e| {
                    Report::new(KernelError::Internal)
                        .attach_printable(format!("RFC 9421 signing failed: {e}"))
                })?;

        let rfc9421_headers = extract_headers(&signed_rfc9421);

        Ok(HttpSigningResponse {
            cavage_headers,
            rfc9421_headers,
        })
    }
}

#[derive(Debug, Clone)]
pub struct HttpSignatureVerifierImpl {
    client: reqwest::Client,
    max_response_bytes: usize,
    date_tolerance: Duration,
    static_actor_keys: HashMap<String, ActorPublicKey>,
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

        let body = self.fetch_limited_json(url).await?;
        actor_public_key_from_json(key_id, &body).map_err(|message| {
            Report::new(KernelError::Rejected).attach_printable(format!(
                "KeyFetchFailed: actor document does not contain a usable public key: {message}"
            ))
        })
    }
}

impl HttpSignatureVerifierImpl {
    async fn fetch_limited_json(&self, mut url: reqwest::Url) -> Result<Value, KernelError> {
        for _ in 0..=5 {
            let resolved_addresses = validate_fetch_url(&url).await?;

            let response = self
                .client_for_url(&url, &resolved_addresses)?
                .get(url.clone())
                .header(
                    ACCEPT,
                    "application/activity+json, application/ld+json; profile=\"https://www.w3.org/ns/activitystreams\"",
                )
                .header(USER_AGENT, "Emumet/0.1 ActivityPub HTTP Signature verifier")
                .send()
                .await
                .map_err(|e| {
                    Report::new(KernelError::Rejected)
                        .attach_printable(format!("KeyFetchFailed: actor key fetch failed: {e}"))
                })?;

            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| {
                        Report::new(KernelError::Rejected)
                            .attach_printable("KeyFetchFailed: redirect without Location header")
                    })?;
                url = url.join(location).map_err(|e| {
                    Report::new(KernelError::Rejected)
                        .attach_printable(format!("KeyFetchFailed: malformed redirect URL: {e}"))
                })?;
                continue;
            }

            if !response.status().is_success() {
                let status = response.status();
                let body_text = response.text().await.unwrap_or_default();
                tracing::debug!(
                    key_fetch_status = %status,
                    key_fetch_body = %body_text,
                    "KeyFetch failed with non-success status"
                );
                return Err(Report::new(KernelError::Rejected).attach_printable(format!(
                    "KeyFetchFailed: actor key endpoint returned {}",
                    status,
                )));
            }

            let mut bytes = Vec::new();
            let mut response = response;
            while let Some(chunk) = response.chunk().await.map_err(|e| {
                Report::new(KernelError::Rejected)
                    .attach_printable(format!("KeyFetchFailed: response read failed: {e}"))
            })? {
                if bytes.len() + chunk.len() > self.max_response_bytes {
                    return Err(Report::new(KernelError::Rejected)
                        .attach_printable("KeyFetchFailed: actor key response exceeds 1 MiB"));
                }
                bytes.extend_from_slice(&chunk);
            }

            return serde_json::from_slice(&bytes).map_err(|e| {
                Report::new(KernelError::Rejected)
                    .attach_printable(format!("KeyFetchFailed: invalid actor JSON: {e}"))
            });
        }

        Err(Report::new(KernelError::Rejected)
            .attach_printable("KeyFetchFailed: too many redirects while fetching actor key"))
    }

    fn client_for_url(
        &self,
        url: &reqwest::Url,
        resolved_addresses: &[SocketAddr],
    ) -> Result<reqwest::Client, KernelError> {
        let Some(host) = url.host_str() else {
            return Ok(self.client.clone());
        };

        if host.parse::<IpAddr>().is_ok() {
            return Ok(self.client.clone());
        }

        let builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10));
        #[cfg(any(test, feature = "test-mode"))]
        let builder = if std::env::var("AP_TEST_ACCEPT_INVALID_CERTS").as_deref() == Ok("1") {
            builder.danger_accept_invalid_certs(true)
        } else {
            builder
        };
        builder
            .resolve_to_addrs(host, resolved_addresses)
            .build()
            .map_err(|e| {
                Report::new(KernelError::Internal)
                    .attach_printable(format!("Failed to build pinned HTTP client: {e}"))
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CavageSignature {
    key_id: String,
    algorithm: String,
    headers: Vec<String>,
    signature: Vec<u8>,
}

fn parse_cavage_signature(header: &str) -> std::result::Result<CavageSignature, String> {
    let mut params = HashMap::new();
    for part in split_signature_params(header)? {
        let (name, value) = parse_signature_param(&part)?;
        if params.insert(name.to_ascii_lowercase(), value).is_some() {
            return Err("duplicate signature parameter is not allowed".to_string());
        }
    }

    let key_id = params
        .remove("keyid")
        .ok_or_else(|| "keyId parameter is required".to_string())?;
    let algorithm = params
        .remove("algorithm")
        .unwrap_or_else(|| "rsa-sha256".to_string());
    let headers = params
        .remove("headers")
        .map(|value| {
            value
                .split_whitespace()
                .map(|header| header.to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .filter(|headers| !headers.is_empty())
        .unwrap_or_else(|| vec!["date".to_string()]);
    let signature = params
        .remove("signature")
        .ok_or_else(|| "signature parameter is required".to_string())?;
    let signature = general_purpose::STANDARD
        .decode(signature.as_bytes())
        .map_err(|e| format!("signature is not valid base64: {e}"))?;

    Ok(CavageSignature {
        key_id,
        algorithm,
        headers,
        signature,
    })
}

fn split_signature_params(header: &str) -> std::result::Result<Vec<String>, String> {
    let mut params = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut escaped = false;

    for ch in header.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' if in_quotes => {
                current.push(ch);
                escaped = true;
            }
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            ',' if !in_quotes => {
                if !current.trim().is_empty() {
                    params.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    if in_quotes {
        return Err("unterminated quoted parameter".to_string());
    }

    if !current.trim().is_empty() {
        params.push(current.trim().to_string());
    }

    if params.is_empty() {
        return Err("Signature header is empty".to_string());
    }

    Ok(params)
}

fn parse_signature_param(part: &str) -> std::result::Result<(String, String), String> {
    let (name, value) = part
        .split_once('=')
        .ok_or_else(|| format!("parameter is missing '=': {part}"))?;
    let name = name.trim();
    if name.is_empty() {
        return Err("parameter name is empty".to_string());
    }

    let value = value.trim();
    let value = if value.starts_with('"') {
        parse_quoted_value(value)?
    } else {
        value.to_string()
    };

    Ok((name.to_string(), value))
}

fn parse_quoted_value(value: &str) -> std::result::Result<String, String> {
    if !value.ends_with('"') || value.len() < 2 {
        return Err("quoted parameter is not closed".to_string());
    }

    let inner = &value[1..value.len() - 1];
    let mut output = String::new();
    let mut escaped = false;

    for ch in inner.chars() {
        if escaped {
            output.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            output.push(ch);
        }
    }

    if escaped {
        return Err("quoted parameter ends with an escape".to_string());
    }

    Ok(output)
}

fn cavage_signing_string(
    request: &HttpSignatureVerificationInput,
    headers: &[String],
) -> std::result::Result<Vec<u8>, String> {
    let url = reqwest::Url::parse(&request.url)
        .map_err(|e| format!("MalformedSignature: request URL is invalid: {e}"))?;
    let path_and_query = match url.query() {
        Some(query) => format!("{}?{}", url.path(), query),
        None => url.path().to_string(),
    };

    let mut lines = Vec::with_capacity(headers.len());
    for header in headers {
        if header == "(request-target)" {
            lines.push(format!(
                "(request-target): {} {}",
                request.method.to_ascii_lowercase(),
                path_and_query
            ));
            continue;
        }

        let value = get_header(&request.headers, header).ok_or_else(|| {
            format!("MalformedSignature: signed header '{header}' is missing from request")
        })?;
        lines.push(format!("{}: {}", header.to_ascii_lowercase(), value));
    }

    Ok(lines.join("\n").into_bytes())
}

fn verify_cavage_signature(
    signing_string: &[u8],
    signature: &[u8],
    public_key_pem: &[u8],
    algorithm: &str,
) -> std::result::Result<bool, String> {
    let algorithm = algorithm.to_ascii_lowercase();

    if algorithm == "ed25519" || algorithm == "ed25519-sha256" {
        return super::crypto::Ed25519Verifier
            .verify(signing_string, signature, public_key_pem)
            .map_err(|e| format!("InvalidSignature: Ed25519 verification failed: {e:?}"));
    }

    if algorithm == "rsa-sha256" || algorithm == "hs2019" {
        match super::crypto::Rsa2048Verifier.verify(signing_string, signature, public_key_pem) {
            Ok(valid) => return Ok(valid),
            Err(rsa_error) if algorithm == "hs2019" => {
                return super::crypto::Ed25519Verifier
                    .verify(signing_string, signature, public_key_pem)
                    .map_err(|ed_error| {
                        format!(
                            "InvalidSignature: hs2019 verification failed: RSA {rsa_error:?}; Ed25519 {ed_error:?}"
                        )
                    });
            }
            Err(e) => {
                return Err(format!("InvalidSignature: RSA verification failed: {e:?}"));
            }
        }
    }

    Err(format!(
        "MalformedSignature: unsupported signature algorithm '{algorithm}'"
    ))
}

fn validate_date_header(
    request: &HttpSignatureVerificationInput,
    tolerance: Duration,
) -> std::result::Result<(), String> {
    let date = get_header(&request.headers, "date")
        .ok_or_else(|| "StaleDate: Date header is required".to_string())?;
    let parsed = httpdate::parse_http_date(date)
        .map_err(|e| format!("StaleDate: Date header is malformed: {e}"))?;
    let now = SystemTime::now();

    if parsed > now {
        let delta = parsed
            .duration_since(now)
            .unwrap_or_else(|_| Duration::from_secs(0));
        if delta > tolerance {
            return Err("StaleDate: Date header is too far in the future".to_string());
        }
    } else {
        let delta = now
            .duration_since(parsed)
            .unwrap_or_else(|_| Duration::from_secs(0));
        if delta > tolerance {
            return Err("StaleDate: Date header is expired".to_string());
        }
    }

    Ok(())
}

fn validate_digest_header(
    request: &HttpSignatureVerificationInput,
) -> std::result::Result<(), String> {
    let has_body = request.body.as_ref().is_some_and(|body| !body.is_empty());
    let Some(digest_header) = get_header(&request.headers, "digest") else {
        if has_body {
            return Err(
                "DigestMismatch: Digest header is required for requests with a body".to_string(),
            );
        }
        return Ok(());
    };
    let expected = digest_header
        .split(',')
        .find_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            if name.eq_ignore_ascii_case("sha-256") {
                Some(value.trim())
            } else {
                None
            }
        })
        .ok_or_else(|| "DigestMismatch: Digest header is missing SHA-256 value".to_string())?;

    let expected = general_purpose::STANDARD
        .decode(expected.as_bytes())
        .map_err(|e| format!("DigestMismatch: SHA-256 digest is not valid base64: {e}"))?;
    let actual = sha2::Sha256::digest(request.body.as_deref().unwrap_or_default());

    if expected.as_slice() == actual.as_slice() {
        Ok(())
    } else {
        Err("DigestMismatch: body digest does not match Digest header".to_string())
    }
}

fn validate_required_signed_headers(
    request: &HttpSignatureVerificationInput,
    signed_headers: &[String],
) -> std::result::Result<(), String> {
    for required in ["(request-target)", "host", "date"] {
        if !signed_headers.iter().any(|header| header == required) {
            return Err(format!(
                "MalformedSignature: required header '{required}' must be signed"
            ));
        }
    }

    let has_body = request.body.as_ref().is_some_and(|body| !body.is_empty());
    if (has_body || get_header(&request.headers, "digest").is_some())
        && !signed_headers.iter().any(|header| header == "digest")
    {
        return Err(
            "MalformedSignature: Digest header must be signed when a body is present".to_string(),
        );
    }

    Ok(())
}

fn get_header<'a>(headers: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Checks whether a host is allowlisted for test-mode AP key fetch operations.
///
/// Reads the `AP_TEST_ALLOWED_FETCH_HOSTS` environment variable (comma-separated,
/// trimmed, lowercase) and returns true if `host_lc` matches any entry.
/// Returns false when the env var is unset or empty.
fn is_fetch_host_allowed(host_lc: &str) -> bool {
    std::env::var("AP_TEST_ALLOWED_FETCH_HOSTS")
        .ok()
        .is_some_and(|val| {
            val.split(',')
                .any(|entry| entry.trim().eq_ignore_ascii_case(host_lc))
        })
}

async fn validate_fetch_url(url: &reqwest::Url) -> Result<Vec<SocketAddr>, KernelError> {
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(Report::new(KernelError::Rejected).attach_printable(format!(
                "SsrfBlocked: unsupported keyId URL scheme '{scheme}'"
            )));
        }
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(Report::new(KernelError::Rejected)
            .attach_printable("SsrfBlocked: keyId URL credentials are not allowed"));
    }

    let host = url.host_str().ok_or_else(|| {
        Report::new(KernelError::Rejected).attach_printable("SsrfBlocked: keyId URL host is empty")
    })?;
    let host_lc = host.trim_end_matches('.').to_ascii_lowercase();

    let ssrf_bypassed = cfg!(any(test, feature = "test-mode")) && is_fetch_host_allowed(&host_lc);

    if !ssrf_bypassed {
        if host_lc == "localhost" || host_lc.ends_with(".localhost") {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("SsrfBlocked: localhost keyId URL is not allowed"));
        }

        if let Ok(ip) = host_lc.parse::<IpAddr>() {
            validate_public_ip(ip)?;
            let port = url.port_or_known_default().ok_or_else(|| {
                Report::new(KernelError::Rejected)
                    .attach_printable("SsrfBlocked: keyId URL does not have a usable port")
            })?;
            return Ok(vec![SocketAddr::new(ip, port)]);
        }

        let port = url.port_or_known_default().ok_or_else(|| {
            Report::new(KernelError::Rejected)
                .attach_printable("SsrfBlocked: keyId URL does not have a usable port")
        })?;
        let addresses = tokio::net::lookup_host((host_lc.as_str(), port))
            .await
            .map_err(|e| {
                Report::new(KernelError::Rejected)
                    .attach_printable(format!("SsrfBlocked: DNS resolution failed: {e}"))
            })?
            .collect::<Vec<_>>();

        if addresses.is_empty() {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("SsrfBlocked: DNS resolution returned no addresses"));
        }

        for address in &addresses {
            validate_public_ip(address.ip())?;
        }

        Ok(addresses)
    } else {
        let port = url.port_or_known_default().ok_or_else(|| {
            Report::new(KernelError::Rejected)
                .attach_printable("SsrfBlocked: keyId URL does not have a usable port")
        })?;
        let addresses = tokio::net::lookup_host((host_lc.as_str(), port))
            .await
            .map_err(|e| {
                Report::new(KernelError::Rejected)
                    .attach_printable(format!("SsrfBlocked: DNS resolution failed: {e}"))
            })?
            .collect::<Vec<_>>();

        if addresses.is_empty() {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("SsrfBlocked: DNS resolution returned no addresses"));
        }
        Ok(addresses)
    }
}

fn validate_public_ip(ip: IpAddr) -> Result<(), KernelError> {
    let blocked = match ip {
        IpAddr::V4(ip) => is_blocked_ipv4(ip),
        IpAddr::V6(ip) => is_blocked_ipv6(ip),
    };

    if blocked {
        Err(Report::new(KernelError::Rejected).attach_printable(format!(
            "SsrfBlocked: non-public IP address is not allowed: {ip}"
        )))
    } else {
        Ok(())
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || ip.is_unspecified()
        || octets[0] == 0
        || octets[0] >= 224
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 198 && (18..=19).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(ipv4) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(ipv4);
    }

    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (ip.segments()[0] & 0xfe00) == 0xfc00
        || (ip.segments()[0] & 0xffc0) == 0xfe80
        || (ip.segments()[0] & 0xffff) == 0x2001 && (ip.segments()[1] & 0xfff0) == 0x0db8
        || ip.segments()[0] == 0x2002
        || (ip.segments()[0] == 0x2001 && ip.segments()[1] == 0)
}

fn actor_public_key_from_json(
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

fn build_http_request(
    request: &HttpSigningRequest,
) -> Result<http::Request<Full<Bytes>>, KernelError> {
    let mut builder = http::Request::builder()
        .method(request.method.as_str())
        .uri(&request.url);

    for (name, value) in &request.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }

    let body = Full::new(Bytes::from(request.body.clone().unwrap_or_default()));

    builder.body(body).map_err(|e| {
        Report::new(KernelError::Internal)
            .attach_printable(format!("Failed to build HTTP request: {e}"))
    })
}

fn extract_headers<B>(req: &http::Request<B>) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    for (name, value) in req.headers() {
        if let Ok(v) = value.to_str() {
            headers.insert(name.to_string(), v.to_string());
        }
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::interfaces::crypto::SigningAlgorithm;

    fn generate_test_rsa_keypair() -> Vec<u8> {
        generate_test_rsa_keypair_with_public().0
    }

    fn generate_test_rsa_keypair_with_public() -> (Vec<u8>, String) {
        use rand::rngs::OsRng;
        use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};

        let private_key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let public_key = rsa::RsaPublicKey::from(&private_key);
        let pem = private_key
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap();
        let public_pem = public_key
            .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap();
        (pem.as_bytes().to_vec(), public_pem)
    }

    fn make_signing_request() -> HttpSigningRequest {
        let mut headers = HashMap::new();
        headers.insert("host".to_string(), "example.com".to_string());
        headers.insert(
            "date".to_string(),
            "Thu, 01 Jan 2025 00:00:00 GMT".to_string(),
        );

        HttpSigningRequest {
            method: "POST".to_string(),
            url: "https://example.com/inbox".to_string(),
            headers,
            body: Some(b"hello".to_vec()),
        }
    }

    fn make_fresh_signing_request(body: &[u8]) -> HttpSigningRequest {
        let mut headers = HashMap::new();
        headers.insert("host".to_string(), "example.com".to_string());
        headers.insert(
            "date".to_string(),
            httpdate::fmt_http_date(SystemTime::now()),
        );

        HttpSigningRequest {
            method: "POST".to_string(),
            url: "https://example.com/inbox?x=1".to_string(),
            headers,
            body: Some(body.to_vec()),
        }
    }

    fn digest_header(body: &[u8]) -> String {
        format!(
            "SHA-256={}",
            general_purpose::STANDARD.encode(sha2::Sha256::digest(body))
        )
    }

    fn test_verifier(key_id: &str, public_key_pem: String) -> HttpSignatureVerifierImpl {
        let mut verifier = HttpSignatureVerifierImpl::new().unwrap();
        verifier.static_actor_keys.insert(
            key_id.to_string(),
            ActorPublicKey {
                id: key_id.to_string(),
                owner: "https://remote.example/users/alice".to_string(),
                public_key_pem,
            },
        );
        verifier
    }

    fn replace_signed_headers(request: &mut HttpSignatureVerificationInput, headers: &str) {
        let current = request.headers.get("signature").unwrap();
        let parsed = parse_cavage_signature(current).unwrap();
        request.headers.insert(
            "signature".to_string(),
            format!(
                "keyId=\"{}\",algorithm=\"{}\",headers=\"{}\",signature=\"{}\"",
                parsed.key_id,
                parsed.algorithm,
                headers,
                general_purpose::STANDARD.encode(parsed.signature)
            ),
        );
    }

    fn replace_signature_algorithm(request: &mut HttpSignatureVerificationInput, algorithm: &str) {
        let current = request.headers.get("signature").unwrap();
        let parsed = parse_cavage_signature(current).unwrap();
        request.headers.insert(
            "signature".to_string(),
            format!(
                "keyId=\"{}\",algorithm=\"{}\",headers=\"{}\",signature=\"{}\"",
                parsed.key_id,
                algorithm,
                parsed.headers.join(" "),
                general_purpose::STANDARD.encode(parsed.signature)
            ),
        );
    }

    async fn signed_verification_request(
        private_key_pem: &[u8],
        key_id: &str,
        body: &[u8],
    ) -> HttpSignatureVerificationInput {
        let signer = HttpSignerImpl;
        let mut signing_request = make_fresh_signing_request(body);
        signing_request
            .headers
            .insert("digest".to_string(), digest_header(body));
        let signed = signer
            .sign(
                &signing_request,
                private_key_pem,
                key_id,
                &SigningAlgorithm::Rsa2048,
            )
            .await
            .unwrap();

        HttpSignatureVerificationInput {
            method: signing_request.method,
            url: signing_request.url,
            headers: signed.cavage_headers,
            body: Some(body.to_vec()),
        }
    }

    #[tokio::test]
    async fn test_dual_sign_produces_both_headers() {
        let signer = HttpSignerImpl;
        let private_key_pem = generate_test_rsa_keypair();
        let key_id = "https://example.com/users/alice#main-key";
        let request = make_signing_request();

        let result = signer
            .sign(
                &request,
                &private_key_pem,
                key_id,
                &SigningAlgorithm::Rsa2048,
            )
            .await;

        assert!(result.is_ok(), "Signing should succeed: {:?}", result.err());
        let response = result.unwrap();

        assert!(
            response.cavage_headers.contains_key("signature"),
            "Cavage headers should contain 'signature', got: {:?}",
            response.cavage_headers
        );

        assert!(
            response.rfc9421_headers.contains_key("signature"),
            "RFC 9421 headers should contain 'signature', got: {:?}",
            response.rfc9421_headers
        );
        assert!(
            response.rfc9421_headers.contains_key("signature-input"),
            "RFC 9421 headers should contain 'signature-input', got: {:?}",
            response.rfc9421_headers
        );
    }

    #[tokio::test]
    async fn test_cavage_signature_contains_key_id() {
        let signer = HttpSignerImpl;
        let private_key_pem = generate_test_rsa_keypair();
        let key_id = "https://example.com/users/alice#main-key";
        let request = make_signing_request();

        let response = signer
            .sign(
                &request,
                &private_key_pem,
                key_id,
                &SigningAlgorithm::Rsa2048,
            )
            .await
            .unwrap();

        let sig_header = &response.cavage_headers["signature"];
        assert!(
            sig_header.contains("keyId="),
            "Cavage signature should contain keyId, got: {}",
            sig_header
        );
        assert!(
            sig_header.contains(key_id),
            "Cavage signature should reference key_id, got: {}",
            sig_header
        );
    }

    #[tokio::test]
    async fn test_rfc9421_signature_input_contains_components() {
        let signer = HttpSignerImpl;
        let private_key_pem = generate_test_rsa_keypair();
        let key_id = "https://example.com/users/alice#main-key";
        let request = make_signing_request();

        let response = signer
            .sign(
                &request,
                &private_key_pem,
                key_id,
                &SigningAlgorithm::Rsa2048,
            )
            .await
            .unwrap();

        let sig_input = &response.rfc9421_headers["signature-input"];
        assert!(
            sig_input.contains("@method"),
            "signature-input should contain @method, got: {}",
            sig_input
        );
        assert!(
            sig_input.contains("@target-uri"),
            "signature-input should contain @target-uri, got: {}",
            sig_input
        );
        assert!(
            sig_input.contains("@authority"),
            "signature-input should contain @authority, got: {}",
            sig_input
        );
    }

    #[tokio::test]
    async fn test_invalid_pem_returns_error() {
        let signer = HttpSignerImpl;
        let bad_pem = b"not-a-valid-pem";
        let key_id = "https://example.com/users/alice#main-key";
        let request = make_signing_request();

        let result = signer
            .sign(&request, bad_pem, key_id, &SigningAlgorithm::Rsa2048)
            .await;

        assert!(result.is_err(), "Invalid PEM should produce an error");
    }

    #[tokio::test]
    async fn test_existing_headers_are_preserved() {
        let signer = HttpSignerImpl;
        let private_key_pem = generate_test_rsa_keypair();
        let key_id = "https://example.com/users/alice#main-key";
        let request = make_signing_request();

        let response = signer
            .sign(
                &request,
                &private_key_pem,
                key_id,
                &SigningAlgorithm::Rsa2048,
            )
            .await
            .unwrap();

        assert!(
            response.cavage_headers.contains_key("host"),
            "Should preserve 'host' header"
        );
        assert!(
            response.cavage_headers.contains_key("date"),
            "Should preserve 'date' header"
        );
    }

    #[test]
    fn test_cavage_signer_key_sign_produces_bytes() {
        let private_key_pem = generate_test_rsa_keypair();
        let key = RsaCavageSignerKey::new(&private_key_pem, "test-key-id").unwrap();

        use http_msgsign_draft::sign::SignerKey;
        let signature = key.sign(b"test signing data");
        assert!(!signature.is_empty(), "Signature should not be empty");
        assert_eq!(key.id(), "test-key-id");
        assert_eq!(key.algorithm(), "rsa-sha256");
    }

    #[test]
    fn test_rfc9421_signer_key_sign_produces_bytes() {
        let private_key_pem = generate_test_rsa_keypair();
        let key = RsaRfc9421SignerKey::new(&private_key_pem, "test-key-id").unwrap();

        use http_msgsign::SignerKey;
        let signature = key.sign(b"test signing data");
        assert!(!signature.is_empty(), "Signature should not be empty");
        assert_eq!(key.key_id(), "test-key-id");
        assert_eq!(RsaRfc9421SignerKey::ALGORITHM, "rsa-v1_5-sha256");
    }

    #[tokio::test]
    async fn test_valid_cavage_signature_passes() {
        let (private_key_pem, public_key_pem) = generate_test_rsa_keypair_with_public();
        let key_id = "https://remote.example/users/alice#main-key";
        let request = signed_verification_request(&private_key_pem, key_id, b"hello").await;
        let verifier = test_verifier(key_id, public_key_pem);

        let result = verifier.verify(&request).await.unwrap();

        assert_eq!(
            result,
            SignatureVerificationResult::Valid {
                key_id: key_id.to_string()
            }
        );
    }

    #[tokio::test]
    async fn test_invalid_cavage_signature_fails() {
        let (private_key_pem, public_key_pem) = generate_test_rsa_keypair_with_public();
        let key_id = "https://remote.example/users/alice#main-key";
        let mut request = signed_verification_request(&private_key_pem, key_id, b"hello").await;
        request
            .headers
            .insert("host".to_string(), "tampered.example".to_string());
        let verifier = test_verifier(key_id, public_key_pem);

        let result = verifier.verify(&request).await.unwrap();

        assert!(
            matches!(result, SignatureVerificationResult::Invalid(message) if message.contains("InvalidSignature"))
        );
    }

    #[tokio::test]
    async fn test_missing_signature_header_fails() {
        let (_private_key_pem, public_key_pem) = generate_test_rsa_keypair_with_public();
        let key_id = "https://remote.example/users/alice#main-key";
        let mut headers = HashMap::new();
        headers.insert(
            "date".to_string(),
            httpdate::fmt_http_date(SystemTime::now()),
        );
        let request = HttpSignatureVerificationInput {
            method: "POST".to_string(),
            url: "https://example.com/inbox".to_string(),
            headers,
            body: Some(b"hello".to_vec()),
        };
        let verifier = test_verifier(key_id, public_key_pem);

        let result = verifier.verify(&request).await.unwrap();

        assert!(
            matches!(result, SignatureVerificationResult::Invalid(message) if message.contains("MissingSignature"))
        );
    }

    #[tokio::test]
    async fn test_digest_mismatch_fails() {
        let (private_key_pem, public_key_pem) = generate_test_rsa_keypair_with_public();
        let key_id = "https://remote.example/users/alice#main-key";
        let mut request = signed_verification_request(&private_key_pem, key_id, b"hello").await;
        request.body = Some(b"tampered".to_vec());
        let verifier = test_verifier(key_id, public_key_pem);

        let result = verifier.verify(&request).await.unwrap();

        assert!(
            matches!(result, SignatureVerificationResult::Invalid(message) if message.contains("DigestMismatch"))
        );
    }

    #[tokio::test]
    async fn test_missing_digest_with_body_fails() {
        let (private_key_pem, public_key_pem) = generate_test_rsa_keypair_with_public();
        let key_id = "https://remote.example/users/alice#main-key";
        let mut request = signed_verification_request(&private_key_pem, key_id, b"hello").await;
        request.headers.remove("digest");
        let verifier = test_verifier(key_id, public_key_pem);

        let result = verifier.verify(&request).await.unwrap();

        assert!(
            matches!(result, SignatureVerificationResult::Invalid(message) if message.contains("DigestMismatch"))
        );
    }

    #[tokio::test]
    async fn test_unsigned_digest_with_body_fails() {
        let (private_key_pem, public_key_pem) = generate_test_rsa_keypair_with_public();
        let key_id = "https://remote.example/users/alice#main-key";
        let mut request = signed_verification_request(&private_key_pem, key_id, b"hello").await;
        replace_signed_headers(&mut request, "(request-target) host date");
        let verifier = test_verifier(key_id, public_key_pem);

        let result = verifier.verify(&request).await.unwrap();

        assert!(
            matches!(result, SignatureVerificationResult::Invalid(message) if message.contains("Digest header must be signed"))
        );
    }

    #[tokio::test]
    async fn test_unsigned_date_fails() {
        let (private_key_pem, public_key_pem) = generate_test_rsa_keypair_with_public();
        let key_id = "https://remote.example/users/alice#main-key";
        let mut request = signed_verification_request(&private_key_pem, key_id, b"hello").await;
        replace_signed_headers(&mut request, "(request-target) host digest");
        let verifier = test_verifier(key_id, public_key_pem);

        let result = verifier.verify(&request).await.unwrap();

        assert!(
            matches!(result, SignatureVerificationResult::Invalid(message) if message.contains("required header 'date'"))
        );
    }

    #[tokio::test]
    async fn test_unsigned_request_target_fails() {
        let (private_key_pem, public_key_pem) = generate_test_rsa_keypair_with_public();
        let key_id = "https://remote.example/users/alice#main-key";
        let mut request = signed_verification_request(&private_key_pem, key_id, b"hello").await;
        replace_signed_headers(&mut request, "host date digest");
        let verifier = test_verifier(key_id, public_key_pem);

        let result = verifier.verify(&request).await.unwrap();

        assert!(
            matches!(result, SignatureVerificationResult::Invalid(message) if message.contains("required header '(request-target)'"))
        );
    }

    #[tokio::test]
    async fn test_unsupported_algorithm_fails() {
        let (private_key_pem, public_key_pem) = generate_test_rsa_keypair_with_public();
        let key_id = "https://remote.example/users/alice#main-key";
        let mut request = signed_verification_request(&private_key_pem, key_id, b"hello").await;
        replace_signature_algorithm(&mut request, "not-rsa");
        let verifier = test_verifier(key_id, public_key_pem);

        let result = verifier.verify(&request).await.unwrap();

        assert!(
            matches!(result, SignatureVerificationResult::Invalid(message) if message.contains("unsupported signature algorithm"))
        );
    }

    #[tokio::test]
    async fn test_expired_date_fails() {
        let (private_key_pem, public_key_pem) = generate_test_rsa_keypair_with_public();
        let key_id = "https://remote.example/users/alice#main-key";
        let signer = HttpSignerImpl;
        let mut signing_request = make_fresh_signing_request(b"hello");
        signing_request.headers.insert(
            "date".to_string(),
            "Thu, 01 Jan 1970 00:00:00 GMT".to_string(),
        );
        signing_request
            .headers
            .insert("digest".to_string(), digest_header(b"hello"));
        let signed = signer
            .sign(
                &signing_request,
                &private_key_pem,
                key_id,
                &SigningAlgorithm::Rsa2048,
            )
            .await
            .unwrap();
        let request = HttpSignatureVerificationInput {
            method: signing_request.method,
            url: signing_request.url,
            headers: signed.cavage_headers,
            body: signing_request.body,
        };
        let verifier = test_verifier(key_id, public_key_pem);

        let result = verifier.verify(&request).await.unwrap();

        assert!(
            matches!(result, SignatureVerificationResult::Invalid(message) if message.contains("StaleDate"))
        );
    }

    #[tokio::test]
    async fn test_ssrf_urls_are_rejected() {
        let localhost = reqwest::Url::parse("http://localhost/actor#main-key").unwrap();
        let loopback = reqwest::Url::parse("http://127.0.0.1/actor#main-key").unwrap();
        let private = reqwest::Url::parse("https://192.168.1.10/actor#main-key").unwrap();
        let mapped_loopback =
            reqwest::Url::parse("http://[::ffff:127.0.0.1]/actor#main-key").unwrap();
        let benchmarking = reqwest::Url::parse("http://198.18.0.1/actor#main-key").unwrap();
        let six_to_four = reqwest::Url::parse("http://[2002:c0a8:0101::1]/actor#main-key").unwrap();

        assert!(validate_fetch_url(&localhost).await.is_err());
        assert!(validate_fetch_url(&loopback).await.is_err());
        assert!(validate_fetch_url(&private).await.is_err());
        assert!(validate_fetch_url(&mapped_loopback).await.is_err());
        assert!(validate_fetch_url(&benchmarking).await.is_err());
        assert!(validate_fetch_url(&six_to_four).await.is_err());
    }

    #[test]
    fn test_duplicate_signature_params_are_rejected() {
        let result = parse_cavage_signature(
            "keyId=\"a\",keyId=\"b\",algorithm=\"rsa-sha256\",signature=\"Zm9v\"",
        );

        assert!(matches!(result, Err(message) if message.contains("duplicate")));
    }

    #[test]
    fn test_actor_public_key_must_match_key_id() {
        let key_id = "https://remote.example/users/alice#main-key";
        let document = serde_json::json!({
            "id": "https://remote.example/users/alice",
            "publicKey": [
                {
                    "id": "https://remote.example/users/alice#other-key",
                    "owner": "https://remote.example/users/alice",
                    "publicKeyPem": "other"
                },
                {
                    "id": key_id,
                    "owner": "https://remote.example/users/alice",
                    "publicKeyPem": "expected"
                }
            ]
        });

        let result = actor_public_key_from_json(key_id, &document).unwrap();

        assert_eq!(result.public_key_pem, "expected");
    }

    #[test]
    fn test_actor_public_key_rejects_mismatched_key_id() {
        let document = serde_json::json!({
            "id": "https://remote.example/users/alice",
            "publicKey": {
                "id": "https://remote.example/users/alice#other-key",
                "owner": "https://remote.example/users/alice",
                "publicKeyPem": "other"
            }
        });

        let result =
            actor_public_key_from_json("https://remote.example/users/alice#main-key", &document);

        assert!(matches!(result, Err(message) if message.contains("does not match keyId")));
    }

    #[tokio::test]
    async fn test_malformed_signature_header_fails() {
        let (_private_key_pem, public_key_pem) = generate_test_rsa_keypair_with_public();
        let key_id = "https://remote.example/users/alice#main-key";
        let mut headers = HashMap::new();
        headers.insert(
            "date".to_string(),
            httpdate::fmt_http_date(SystemTime::now()),
        );
        headers.insert("signature".to_string(), "keyId=\"unterminated".to_string());
        let request = HttpSignatureVerificationInput {
            method: "POST".to_string(),
            url: "https://example.com/inbox".to_string(),
            headers,
            body: Some(b"hello".to_vec()),
        };
        let verifier = test_verifier(key_id, public_key_pem);

        let result = verifier.verify(&request).await.unwrap();

        assert!(
            matches!(result, SignatureVerificationResult::Invalid(message) if message.contains("MalformedSignature"))
        );
    }

    #[tokio::test]
    async fn test_key_fetch_failure_is_invalid_result() {
        let (private_key_pem, _public_key_pem) = generate_test_rsa_keypair_with_public();
        let key_id = "http://127.0.0.1/users/alice#main-key";
        let request = signed_verification_request(&private_key_pem, key_id, b"hello").await;
        let verifier = HttpSignatureVerifierImpl::new().unwrap();

        let result = verifier.verify(&request).await.unwrap();

        assert!(
            matches!(result, SignatureVerificationResult::Invalid(message) if message.contains("KeyFetchFailed"))
        );
    }
}
