use crate::entity::{AccountId, StellarAccountId};
use crate::{
    entity::{Account, AccountName},
    error::KernelError,
};

#[async_trait::async_trait]
pub trait AccountRepository: 'static + Sync + Send {
    async fn find_by_id(&self, id: &AccountId) -> Result<Option<Account>, KernelError>;
    async fn find_by_stellar_id(
        &self,
        stellar_id: &StellarAccountId,
    ) -> Result<Vec<Account>, KernelError>;
    async fn find_by_name(&self, name: &AccountName) -> Result<Option<Account>, KernelError>;
    async fn save(&self, account: &Account) -> Result<(), KernelError>;
    async fn update(&self, account: &Account) -> Result<(), KernelError>;
    async fn delete(&self, id: &AccountId) -> Result<(), KernelError>;
}

pub trait DependOnAccountRepository: 'static + Sync + Send {
    type AccountRepository: AccountRepository;

    fn account_repository(&self) -> &Self::AccountRepository;
}
