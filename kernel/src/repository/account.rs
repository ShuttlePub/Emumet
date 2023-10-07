use crate::{
    entity::{Account, AccountDomain, AccountName, Id, StellarAccount},
    error::KernelError,
};

#[async_trait::async_trait]
pub trait AccountRepository: 'static + Sync + Send {
    async fn find_by_id(&self, id: &Id<Account>) -> Result<Option<Account>, KernelError>;
    async fn find_by_stellar_id(
        &self,
        stellar_id: &Id<StellarAccount>,
    ) -> Result<Vec<Account>, KernelError>;
    async fn find_by_name(&self, name: &AccountName) -> Result<Option<Account>, KernelError>;
    async fn find_by_domain(&self, domain: &AccountDomain) -> Result<Vec<Account>, KernelError>;
    async fn save(&self, account: &Account) -> Result<(), KernelError>;
    async fn update(&self, account: &Account) -> Result<(), KernelError>;
    async fn delete(&self, id: &Id<Account>) -> Result<(), KernelError>;
}

pub trait DependOnAccountRepository: 'static + Sync + Send {
    type AccountRepository: AccountRepository;

    fn account_repository(&self) -> &Self::AccountRepository;
}
