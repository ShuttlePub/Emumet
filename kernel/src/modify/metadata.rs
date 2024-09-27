use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::{Metadata, MetadataId};
use crate::KernelError;
use std::future::Future;

pub trait MetadataModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    fn create(
        &self,
        transaction: &mut Self::Transaction,
        metadata: &Metadata,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        transaction: &mut Self::Transaction,
        metadata: &Metadata,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        transaction: &mut Self::Transaction,
        metadata_id: &MetadataId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnMetadataModifier: Sync + Send + DependOnDatabaseConnection {
    type MetadataModifier: MetadataModifier<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn metadata_modifier(&self) -> &Self::MetadataModifier;
}
