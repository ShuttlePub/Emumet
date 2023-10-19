use crate::entity::{AccountId, MetadataId};
use crate::{entity::Metadata, error::KernelError};

#[async_trait::async_trait]
pub trait MetadataRepository: 'static + Send + Sync {
    async fn find_by_id(&self, id: &MetadataId) -> Result<Option<Metadata>, KernelError>;
    async fn find_by_account_id(
        &self,
        account_id: &AccountId,
    ) -> Result<Vec<Metadata>, KernelError>;
    async fn save(&self, metadata: &Metadata) -> Result<(), KernelError>;
    async fn update(&self, metadata: &Metadata) -> Result<(), KernelError>;
    async fn delete(&self, account_id: &AccountId) -> Result<(), KernelError>;
}

pub trait DependOnMetadataRepository: 'static + Sync + Send {
    type MetadataRepository: MetadataRepository;

    fn metadata_repository(&self) -> &Self::MetadataRepository;
}
