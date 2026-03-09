use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{AccountId, Metadata, MetadataId};
use crate::KernelError;
use std::future::Future;

pub trait MetadataReadModel: Sync + Send + 'static {
    type Executor: Executor;

    // Query operations (projection reads)
    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &MetadataId,
    ) -> impl Future<Output = error_stack::Result<Option<Metadata>, KernelError>> + Send;

    fn find_by_account_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<Metadata>, KernelError>> + Send;

    // Projection update operations (called by EventApplier pipeline)
    fn create(
        &self,
        executor: &mut Self::Executor,
        metadata: &Metadata,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Executor,
        metadata: &Metadata,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Executor,
        metadata_id: &MetadataId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnMetadataReadModel: Sync + Send + DependOnDatabaseConnection {
    type MetadataReadModel: MetadataReadModel<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn metadata_read_model(&self) -> &Self::MetadataReadModel;
}
