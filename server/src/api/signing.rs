use super::resolve_auth_account_id;
use crate::auth::OidcAuthInfo;
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use application::permission::{account_sign, check_permission};
use application::signing_key::{GetPublicKeyUseCase, PublicKeyInfo, SignRequestUseCase};
use axum::extract::FromRef;
use axum::http::StatusCode;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::http_signing::{HttpSigningRequest, HttpSigningResponse};
use kernel::interfaces::read_model::{AccountQuery, DependOnAccountQuery};
use kernel::prelude::entity::{Account, AccountId, AuthAccountId, Nanoid};
use kernel::KernelError;
use std::sync::Arc;

#[derive(Clone)]
pub struct SigningApi {
    module: Arc<AppModule>,
}

impl SigningApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn resolve_auth_account_id(
        &self,
        auth_info: OidcAuthInfo,
    ) -> error_stack::Result<AuthAccountId, KernelError> {
        resolve_auth_account_id(&self.module, auth_info).await
    }

    pub async fn find_account_by_nanoid(&self, id: String) -> Result<Account, ErrorStatus> {
        if id.trim().is_empty() {
            return Err(ErrorStatus::from((
                StatusCode::BAD_REQUEST,
                "Account ID cannot be empty".to_string(),
            )));
        }

        let nanoid = Nanoid::<Account>::new(id);
        let mut executor = self
            .module
            .database_connection()
            .connection()
            .await
            .map_err(ErrorStatus::from)?;
        self.module
            .account_query()
            .find_by_nanoid(&mut executor, &nanoid)
            .await
            .map_err(ErrorStatus::from)?
            .ok_or_else(|| {
                ErrorStatus::from((StatusCode::NOT_FOUND, "Account not found".to_string()))
            })
    }

    pub async fn check_sign_permission(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        check_permission(
            self.module.as_ref(),
            auth_account_id,
            &account_sign(account_id),
        )
        .await
    }

    pub async fn sign(
        &self,
        account_id: &AccountId,
        request: HttpSigningRequest,
    ) -> error_stack::Result<HttpSigningResponse, KernelError> {
        self.module.sign(account_id, request).await
    }

    pub async fn get_public_key_info(
        &self,
        account_id: &AccountId,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<PublicKeyInfo, KernelError> {
        self.module.get_public_key_info(account_id, nanoid).await
    }
}

impl FromRef<AppModule> for SigningApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
