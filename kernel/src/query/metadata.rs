use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::{AccountId, Metadata, MetadataId};
use crate::KernelError;
use std::future::Future;

pub trait MetadataQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        metadata_id: &MetadataId,
    ) -> impl Future<Output = error_stack::Result<Option<Metadata>, KernelError>> + Send;

    fn find_by_account_id(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<Metadata>, KernelError>> + Send;
}

pub trait DependOnMetadataQuery: Sync + Send + DependOnDatabaseConnection {
    type MetadataQuery: MetadataQuery<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn metadata_query(&self) -> &Self::MetadataQuery;
}
