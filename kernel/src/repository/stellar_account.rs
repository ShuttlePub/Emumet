use crate::{
    entity::{AccessToken, Id, RefreshToken, StellarAccount},
    error::KernelError,
};

#[async_trait::async_trait]
pub trait StellarAccountRepository: 'static + Sync + Send {
    async fn find_by_id(&self, id: &Id<StellarAccount>) -> Result<Option<StellarAccount>, KernelError>;
    async fn find_by_refresh_token(
        &self,
        token: &RefreshToken,
    ) -> Result<Option<StellarAccount>, KernelError>;
    async fn find_by_access_token(
        &self,
        token: &AccessToken,
    ) -> Result<Option<StellarAccount>, KernelError>;
    async fn save(&self, stellar_account: &StellarAccount) -> Result<(), KernelError>;
    async fn update(&self, stellar_account: &StellarAccount) -> Result<(), KernelError>;
    async fn delete(&self, stellar_account: &StellarAccount) -> Result<(), KernelError>;
}

pub trait DependOnStellarAccountRepository: 'static + Sync + Send {
    type StellarAccountRepository: StellarAccountRepository;

    fn stellar_account_repository(&self) -> &Self::StellarAccountRepository;
}
