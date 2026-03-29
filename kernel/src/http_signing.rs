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
