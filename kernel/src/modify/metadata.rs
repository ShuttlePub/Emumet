use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::{CommandEnvelope, Metadata, MetadataEvent, MetadataId};
use crate::KernelError;

pub trait MetadataModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn create(
        &self,
        transaction: &mut Self::Transaction,
        metadata: &Metadata,
    ) -> error_stack::Result<(), KernelError>;

    async fn update(
        &self,
        transaction: &mut Self::Transaction,
        metadata: &Metadata,
    ) -> error_stack::Result<(), KernelError>;

    async fn delete(
        &self,
        transaction: &mut Self::Transaction,
        metadata_id: &MetadataId,
    ) -> error_stack::Result<(), KernelError>;
}

pub trait DependOnMetadataModifier: Sync + Send + DependOnDatabaseConnection {
    type MetadataModifier: MetadataModifier<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn metadata_modifier(&self) -> &Self::MetadataModifier;
}

pub trait MetadataEventModifier: 'static + Sync + Send {
    type Transaction: Transaction;

    async fn handle(
        &self,
        transaction: &mut Self::Transaction,
        metadata_id: &MetadataId,
        event: &CommandEnvelope<MetadataEvent, Metadata>,
    ) -> error_stack::Result<(), KernelError>;
}

pub trait DependOnMetadataEventModifier: Sync + Send + DependOnDatabaseConnection {
    type MetadataEventModifier: MetadataEventModifier<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn metadata_event_modifier(&self) -> &Self::MetadataEventModifier;
}
