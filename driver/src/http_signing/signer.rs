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

use super::cavage::get_header;

pub(super) struct RsaCavageSignerKey {
    private_key: RsaPrivateKey,
    key_id: String,
}

impl RsaCavageSignerKey {
    pub(super) fn new(private_key_pem: &[u8], key_id: &str) -> Result<Self, KernelError> {
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

pub(super) struct RsaRfc9421SignerKey {
    private_key: RsaPrivateKey,
    key_id: String,
}

impl RsaRfc9421SignerKey {
    pub(super) fn new(private_key_pem: &[u8], key_id: &str) -> Result<Self, KernelError> {
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
