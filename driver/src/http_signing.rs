use std::collections::HashMap;

use bytes::Bytes;
use error_stack::{Report, Result};
use http_body_util::Full;
use kernel::interfaces::http_signing::{HttpSigner, HttpSigningRequest, HttpSigningResponse};
use kernel::KernelError;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::sha2::Sha256;
use rsa::signature::{SignatureEncoding, Signer as RsaSigner};
use rsa::RsaPrivateKey;

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
        let cavage_params = http_msgsign_draft::sign::SignatureParams::builder()
            .add_request_target()
            .add_header("host")
            .add_header("date")
            .build()
            .map_err(|e| {
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
        use rand::rngs::OsRng;
        use rsa::pkcs8::EncodePrivateKey;

        let private_key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let pem = private_key
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap();
        pem.as_bytes().to_vec()
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
}
