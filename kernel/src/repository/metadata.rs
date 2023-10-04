use crate::{
    entity::{Account, Id, Metadata},
    error::KernelError,
};

pub trait MetadataRepository: 'static + Send + Sync {
    fn find_by_id(&self, id: &Id<Metadata>) -> Result<Option<Metadata>, KernelError>;
    fn find_by_account_id(&self, account_id: &Id<Account>) -> Result<Vec<Metadata>, KernelError>;
    fn save(&self, metadata: &Metadata) -> Result<(), KernelError>;
    fn update(&self, metadata: &Metadata) -> Result<(), KernelError>;
    fn delete(&self, account_id: &Id<Account>) -> Result<(), KernelError>;
}

pub trait DependOnMetadataRepository: 'static + Sync + Send {
    type MetadataRepository: MetadataRepository;

    fn metadata_repository(&self) -> &Self::MetadataRepository;
}
