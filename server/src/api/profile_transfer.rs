use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use application::service::profile_transfer::{
    AcceptProfileTransferRequestUseCase, CancelProfileTransferRequestUseCase,
    RejectProfileTransferRequestUseCase, RequestProfileTransferUseCase,
};
use axum::extract::FromRef;
use kernel::prelude::entity::AuthAccountId;
use kernel::KernelError;
use std::sync::Arc;

#[derive(Clone)]
pub struct ProfileTransferApi {
    module: Arc<AppModule>,
}

impl ProfileTransferApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn resolve_auth_account_id(
        &self,
        auth_info: OidcAuthInfo,
    ) -> error_stack::Result<AuthAccountId, KernelError> {
        resolve_auth_account_id(&self.module, auth_info).await
    }

    pub async fn request_profile_transfer(
        &self,
        auth_account_id: &AuthAccountId,
        profile_nanoid: String,
        org_account_nanoid: String,
    ) -> error_stack::Result<
        application::dto::profile_transfer::ProfileTransferRequestDto,
        KernelError,
    > {
        self.module
            .request_profile_transfer(auth_account_id, profile_nanoid, org_account_nanoid)
            .await
    }

    pub async fn accept_profile_transfer_request(
        &self,
        auth_account_id: &AuthAccountId,
        request_nanoid: String,
    ) -> error_stack::Result<(), KernelError> {
        self.module
            .accept_profile_transfer_request(auth_account_id, request_nanoid)
            .await
    }

    pub async fn reject_profile_transfer_request(
        &self,
        auth_account_id: &AuthAccountId,
        request_nanoid: String,
    ) -> error_stack::Result<(), KernelError> {
        self.module
            .reject_profile_transfer_request(auth_account_id, request_nanoid)
            .await
    }

    pub async fn cancel_profile_transfer_request(
        &self,
        auth_account_id: &AuthAccountId,
        request_nanoid: String,
    ) -> error_stack::Result<(), KernelError> {
        self.module
            .cancel_profile_transfer_request(auth_account_id, request_nanoid)
            .await
    }
}

impl FromRef<AppModule> for ProfileTransferApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
