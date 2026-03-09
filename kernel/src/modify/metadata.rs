use crate::database::{DependOnDatabaseConnection, Executor};
use crate::entity::{Metadata, MetadataId};
use crate::KernelError;
use std::future::Future;

pub trait MetadataModifier: Sync + Send + 'static {
    type Executor: Executor;

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

pub trait DependOnMetadataModifier: Sync + Send + DependOnDatabaseConnection {
    type MetadataModifier: MetadataModifier<
        Executor = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Executor,
    >;

    fn metadata_modifier(&self) -> &Self::MetadataModifier;
}
