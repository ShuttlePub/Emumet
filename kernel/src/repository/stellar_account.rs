use crate::{error::KernelError, entities::StellarAccount};

pub trait StellarAccountRepository: 'static + Sync + Send {
    fn find_by_id(&self, id: impl AsRef<i64>) -> Result<Option<StellarAccount>, KernelError>;
    fn find_by_refresh_token(&self, token: impl AsRef<str>) -> Result<Option<StellarAccount>, KernelError>;
    fn find_by_access_token(&self, token: impl AsRef<str>) -> Result<Option<StellarAccount>, KernelError>;
    fn save(&self, stellar_account: &StellarAccount) -> Result<(), KernelError>;
    fn delete(&self, stellar_account: &StellarAccount) -> Result<(), KernelError>;
}

pub trait DependOnStellarAccountRepository: 'static + Sync + Send {
    type StellarAccountRepository: StellarAccountRepository;

    fn stellar_account_repository(&self) -> &Self::StellarAccountRepository;
}
