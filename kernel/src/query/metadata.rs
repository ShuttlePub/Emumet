use crate::database::{DependOnDatabaseConnection, Executor};
use crate::entity::{AccountId, Metadata, MetadataId};
use crate::KernelError;
use std::future::Future;

pub trait MetadataQuery: Sync + Send + 'static {
    type Executor: Executor;

    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        metadata_id: &MetadataId,
    ) -> impl Future<Output = error_stack::Result<Option<Metadata>, KernelError>> + Send;

    fn find_by_account_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<Metadata>, KernelError>> + Send;
}

pub trait DependOnMetadataQuery: Sync + Send + DependOnDatabaseConnection {
    type MetadataQuery: MetadataQuery<
        Executor = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Executor,
    >;

    fn metadata_query(&self) -> &Self::MetadataQuery;
}
