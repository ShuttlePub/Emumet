use crate::{
    entity::{Account, Id, StellarAccount},
    error::KernelError,
};

pub trait StellarEmumetAccountRepository: 'static + Sync + Send {
    fn find_by_stellar_id(&self, id: &Id<StellarAccount>) -> Result<Vec<Account>, KernelError>;
    fn find_by_emumet_id(&self, id: &Id<Account>) -> Result<Vec<StellarAccount>, KernelError>;
    fn save(
        &self,
        stellar_id: &Id<StellarAccount>,
        emumet_id: &Id<Account>,
    ) -> Result<(), KernelError>;
    fn delete(
        &self,
        stellar_id: &Id<StellarAccount>,
        emumet_id: &Id<Account>,
    ) -> Result<(), KernelError>;
}

pub trait DependOnStellarEmumetAccountRepository: 'static + Sync + Send {
    type StellarEmumetAccountRepository: StellarEmumetAccountRepository;

    fn stellar_emumet_account_repository(&self) -> &Self::StellarEmumetAccountRepository;
}
