use crate::error::ErrorStatus;
use crate::handler::AppModule;
use application::dto::activitypub::{GetProfileActorDto, GetProfileWebFingerDto};
use application::dto::profile_inbound::ProfileInboxOutcome;
use application::instance_identity::GetInstanceActorUseCase;
use application::service::activitypub::{GetProfileActorUseCase, GetProfileWebFingerUseCase};
use axum::extract::FromRef;
use axum::http::StatusCode;
use kernel::activitypub::{Actor, InstanceActor, WebFingerResponse};
use kernel::interfaces::http_signing::{
    DependOnHttpSignatureVerifier, HttpSignatureVerificationInput, HttpSignatureVerifier,
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

    pub fn relay_inbox_enabled(&self) -> bool {
        self.module.post_relay_inbox_enabled()
    }
    pub async fn admit_profile_inbox(
        &self,
        dto: application::service::activitypub::ProfileRelayInboxDto,
    ) -> error_stack::Result<ProfileInboxOutcome, KernelError> {
        use application::service::activitypub::ProfileRelayInboxUseCase;
        self.module.inbox_module().admit_profile_inbox(dto).await
    }
    pub async fn get_profile_actor(
        &self,
        dto: GetProfileActorDto,
    ) -> error_stack::Result<Actor, KernelError> {
        self.module.get_profile_actor(dto).await
    }

    /// The public instance actor document (`{PUBLIC_BASE_URL}/ap/instance`).
    pub async fn get_instance_actor(&self) -> error_stack::Result<InstanceActor, KernelError> {
        self.module.get_instance_actor().await
    }

    pub async fn get_profile_webfinger(
        &self,
        dto: GetProfileWebFingerDto,
    ) -> error_stack::Result<WebFingerResponse, KernelError> {
        self.module.get_profile_webfinger(dto).await
    }

    pub async fn verify_inbox_signature(
        &self,
        input: &HttpSignatureVerificationInput,
    ) -> error_stack::Result<kernel::interfaces::http_signing::SignatureEvidenceResult, KernelError>
    {
        self.module
            .http_signature_verifier()
            .verify_with_evidence(input)
            .await
    }
}

impl FromRef<AppModule> for ActivityPubApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
