use crate::{entities::Account, error::KernelError};

pub trait AccountRepository: 'static + Sync + Send {
    fn find_by_id(&self, id: impl AsRef<i64>) -> Result<Option<Account>, KernelError>;
    fn find_by_stellar_id(
        &self,
        stellar_id: impl AsRef<str>,
    ) -> Result<Option<Account>, KernelError>;
    fn find_by_name(&self, name: impl AsRef<str>) -> Result<Option<Account>, KernelError>;
    fn save(&self, account: &Account) -> Result<(), KernelError>;
    fn delete(&self, account: &Account) -> Result<(), KernelError>;
}

pub trait DependOnAccountRepository: 'static + Sync + Send {
    type AccountRepository: AccountRepository;

    fn account_repository(&self) -> &Self::AccountRepository;
}
