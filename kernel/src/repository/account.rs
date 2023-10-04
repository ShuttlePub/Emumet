use crate::{
    entity::{Account, AccountName, Id},
    error::KernelError,
};

pub trait AccountRepository: 'static + Sync + Send {
    fn find_by_id(&self, id: &Id<Account>) -> Result<Option<Account>, KernelError>;
    fn find_by_stellar_id(&self, stellar_id: &Id<Account>) -> Result<Option<Account>, KernelError>;
    fn find_by_name(&self, name: &AccountName) -> Result<Option<Account>, KernelError>;
    fn save(&self, account: &Account) -> Result<(), KernelError>;
    fn update(&self, account: &Account) -> Result<(), KernelError>;
    fn delete(&self, id: &Id<Account>) -> Result<(), KernelError>;
}

pub trait DependOnAccountRepository: 'static + Sync + Send {
    type AccountRepository: AccountRepository;

    fn account_repository(&self) -> &Self::AccountRepository;
}
