use crate::entity::{AccountId, StellarAccountId};
use crate::{
    entity::{Account, StellarAccount},
    error::KernelError,
};

#[async_trait::async_trait]
pub trait StellarEmumetAccountRepository: 'static + Sync + Send {
    async fn find_by_stellar_id(&self, id: &StellarAccountId) -> Result<Vec<Account>, KernelError>;
    async fn find_by_emumet_id(&self, id: &AccountId) -> Result<Vec<StellarAccount>, KernelError>;
    async fn save(
        &self,
        stellar_id: &StellarAccountId,
        emumet_id: &AccountId,
    ) -> Result<(), KernelError>;
    async fn delete(
        &self,
        stellar_id: &StellarAccountId,
        emumet_id: &AccountId,
    ) -> Result<(), KernelError>;
}

pub trait DependOnStellarEmumetAccountRepository: 'static + Sync + Send {
    type StellarEmumetAccountRepository: StellarEmumetAccountRepository;

    fn stellar_emumet_account_repository(&self) -> &Self::StellarEmumetAccountRepository;
}
