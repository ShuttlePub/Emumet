use crate::{
    entity::{AccessToken, Id, RefreshToken, StellarAccount},
    error::KernelError,
};

pub trait StellarAccountRepository: 'static + Sync + Send {
    fn find_by_id(&self, id: &Id<StellarAccount>) -> Result<Option<StellarAccount>, KernelError>;
    fn find_by_refresh_token(
        &self,
        token: &RefreshToken,
    ) -> Result<Option<StellarAccount>, KernelError>;
    fn find_by_access_token(
        &self,
        token: &AccessToken,
    ) -> Result<Option<StellarAccount>, KernelError>;
    fn save(&self, stellar_account: &StellarAccount) -> Result<(), KernelError>;
    fn update(&self, stellar_account: &StellarAccount) -> Result<(), KernelError>;
    fn delete(&self, stellar_account: &StellarAccount) -> Result<(), KernelError>;
}

pub trait DependOnStellarAccountRepository: 'static + Sync + Send {
    type StellarAccountRepository: StellarAccountRepository;

    fn stellar_account_repository(&self) -> &Self::StellarAccountRepository;
}
