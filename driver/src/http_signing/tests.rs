use std::collections::HashMap;
use std::time::SystemTime;

use base64::{engine::general_purpose, Engine as _};
use kernel::interfaces::crypto::SigningAlgorithm;
use kernel::interfaces::http_signing::{
    ActorPublicKey, HttpSignatureVerificationInput, HttpSignatureVerifier, HttpSigner,
    HttpSigningRequest, SignatureVerificationResult,
};
use rsa::RsaPrivateKey;
use sha2::Digest;

use super::actor_key::actor_public_key_from_json;
use super::cavage::parser::parse_cavage_signature;
use super::signer::{RsaCavageSignerKey, RsaRfc9421SignerKey};
use super::ssrf::validate_fetch_url;
use super::{HttpSignatureVerifierImpl, HttpSignerImpl};

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
    let mapped_loopback = reqwest::Url::parse("http://[::ffff:127.0.0.1]/actor#main-key").unwrap();
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
