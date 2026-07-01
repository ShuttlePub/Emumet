use crate::crypto::SigningAlgorithm;
use crate::KernelError;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct HttpSigningRequest {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct HttpSigningResponse {
    pub cavage_headers: HashMap<String, String>,
    pub rfc9421_headers: HashMap<String, String>,
}

pub trait HttpSigner: Send + Sync {
    fn sign(
        &self,
        request: &HttpSigningRequest,
        private_key_pem: &[u8],
        key_id: &str,
        algorithm: &SigningAlgorithm,
    ) -> impl std::future::Future<Output = error_stack::Result<HttpSigningResponse, KernelError>> + Send;
}

pub trait DependOnHttpSigner: Send + Sync {
    type HttpSigner: HttpSigner;
    fn http_signer(&self) -> &Self::HttpSigner;
}

#[derive(Debug, Clone)]
pub struct HttpSignatureVerificationInput {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SignatureScheme {
    Cavage,
    Rfc9421,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ActorPublicKey {
    pub id: String,
    pub owner: String,
    pub public_key_pem: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SignatureVerificationResult {
    Valid { key_id: String },
    Invalid(String),
}

pub trait HttpSignatureVerifier: Send + Sync {
    fn verify(
        &self,
        request: &HttpSignatureVerificationInput,
    ) -> impl std::future::Future<
        Output = error_stack::Result<SignatureVerificationResult, KernelError>,
    > + Send;

    fn fetch_actor_key(
        &self,
        key_id: &str,
    ) -> impl std::future::Future<Output = error_stack::Result<ActorPublicKey, KernelError>> + Send;
}

pub trait DependOnHttpSignatureVerifier: Send + Sync {
    type HttpSignatureVerifier: HttpSignatureVerifier;
    fn http_signature_verifier(&self) -> &Self::HttpSignatureVerifier;
}
