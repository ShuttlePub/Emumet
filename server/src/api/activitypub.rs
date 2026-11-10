use crate::error::ErrorStatus;
use crate::handler::AppModule;
use application::dto::activitypub::{GetProfileActorDto, GetProfileWebFingerDto};
use application::dto::profile_inbound::{ProfileInboxDto, ProfileInboxOutcome};
use application::service::activitypub::{
    GetProfileActorUseCase, GetProfileWebFingerUseCase, ProfileInboxUseCase,
};
use axum::extract::FromRef;
use axum::http::StatusCode;
use kernel::activitypub::{Actor, WebFingerResponse};
use kernel::interfaces::http_signing::{
    ActorPublicKey, DependOnHttpSignatureVerifier, HttpSignatureVerificationInput,
    HttpSignatureVerifier, SignatureVerificationResult,
};
use kernel::KernelError;
use std::sync::Arc;

#[derive(Clone)]
pub struct ActivityPubApi {
    module: Arc<AppModule>,
}

impl ActivityPubApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub fn public_base_url(&self) -> &kernel::interfaces::config::PublicBaseUrl {
        use kernel::interfaces::config::DependOnPublicBaseUrl;
        self.module.public_base_url()
    }

    pub fn public_base_host_header(&self) -> Result<String, ErrorStatus> {
        let base_url = self.public_base_url().as_str();
        let url = url::Url::parse(base_url).map_err(|e| {
            ErrorStatus::from((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Invalid PUBLIC_BASE_URL: {e}"),
            ))
        })?;
        let host = url.host_str().ok_or_else(|| {
            ErrorStatus::from((
                StatusCode::INTERNAL_SERVER_ERROR,
                "PUBLIC_BASE_URL must include a host".to_string(),
            ))
        })?;
        Ok(match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        })
    }

    pub async fn get_profile_actor(
        &self,
        dto: GetProfileActorDto,
    ) -> error_stack::Result<Actor, KernelError> {
        self.module.get_profile_actor(dto).await
    }

    pub async fn get_profile_webfinger(
        &self,
        dto: GetProfileWebFingerDto,
    ) -> error_stack::Result<WebFingerResponse, KernelError> {
        self.module.get_profile_webfinger(dto).await
    }

    pub async fn verify_http_signature(
        &self,
        input: &HttpSignatureVerificationInput,
    ) -> error_stack::Result<SignatureVerificationResult, KernelError> {
        self.module.http_signature_verifier().verify(input).await
    }

    pub async fn fetch_actor_key(
        &self,
        key_id: &str,
    ) -> error_stack::Result<ActorPublicKey, KernelError> {
        self.module
            .http_signature_verifier()
            .fetch_actor_key(key_id)
            .await
    }

    pub async fn handle_profile_inbox_activity(
        &self,
        dto: ProfileInboxDto,
    ) -> error_stack::Result<ProfileInboxOutcome, KernelError> {
        self.module.handle_profile_inbox_activity(dto).await
    }
}

impl FromRef<AppModule> for ActivityPubApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
