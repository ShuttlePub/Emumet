use crate::error::ErrorStatus;
use crate::handler::AppModule;
use application::dto::activitypub::{GetActorDto, GetWebFingerDto, InboxActivityDto};
use application::service::activitypub::{
    GetActorUseCase, GetFollowersCollectionUseCase, GetOutboxUseCase, GetWebFingerUseCase,
    InboxUseCase,
};
use axum::extract::FromRef;
use axum::http::StatusCode;
use kernel::activitypub::{Actor, OrderedCollection, WebFingerResponse};
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::http_signing::{
    ActorPublicKey, DependOnHttpSignatureVerifier, HttpSignatureVerificationInput,
    HttpSignatureVerifier, SignatureVerificationResult,
};
use kernel::interfaces::read_model::{AccountQuery, DependOnAccountQuery};
use kernel::prelude::entity::{Account, AccountId, Nanoid};
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

    pub async fn find_account_id_by_nanoid(&self, id: String) -> Result<AccountId, ErrorStatus> {
        if id.trim().is_empty() {
            return Err(ErrorStatus::from((
                StatusCode::BAD_REQUEST,
                "Account ID cannot be empty".to_string(),
            )));
        }

        let mut executor = self
            .module
            .database_connection()
            .connection()
            .await
            .map_err(ErrorStatus::from)?;
        let nanoid = Nanoid::<Account>::new(id);
        self.module
            .account_query()
            .find_by_nanoid(&mut executor, &nanoid)
            .await
            .map_err(ErrorStatus::from)?
            .map(|account| account.id().clone())
            .ok_or_else(|| ErrorStatus::from(StatusCode::NOT_FOUND))
    }

    pub async fn get_actor(&self, dto: GetActorDto) -> error_stack::Result<Actor, KernelError> {
        self.module.get_actor(dto).await
    }

    pub async fn get_followers_collection(
        &self,
        account_id: &AccountId,
    ) -> error_stack::Result<OrderedCollection, KernelError> {
        self.module.get_followers_collection(account_id).await
    }

    pub async fn get_following_collection(
        &self,
        account_id: &AccountId,
    ) -> error_stack::Result<OrderedCollection, KernelError> {
        self.module.get_following_collection(account_id).await
    }

    pub async fn get_outbox_collection(
        &self,
        account_id: &AccountId,
        limit: usize,
        cursor: Option<i64>,
    ) -> error_stack::Result<OrderedCollection, KernelError> {
        self.module
            .get_outbox_collection(account_id, limit, cursor)
            .await
    }

    pub async fn get_webfinger(
        &self,
        dto: GetWebFingerDto,
    ) -> error_stack::Result<WebFingerResponse, KernelError> {
        self.module.get_webfinger(dto).await
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

    pub async fn handle_inbox_activity(
        &self,
        dto: InboxActivityDto,
    ) -> error_stack::Result<(), KernelError> {
        self.module.handle_inbox_activity(dto).await
    }
}

impl FromRef<AppModule> for ActivityPubApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
