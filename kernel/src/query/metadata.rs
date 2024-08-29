use crate::database::Transaction;
use crate::entity::{AccountId, EventEnvelope, EventVersion, Metadata, MetadataEvent, MetadataId};
use crate::KernelError;

pub trait MetadataQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        metadata_id: &MetadataId,
    ) -> error_stack::Result<Option<Metadata>, KernelError>;

    async fn find_by_account_id(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
    ) -> error_stack::Result<Vec<Metadata>, KernelError>;
}

pub trait DependOnMetadataQuery: Sync + Send {
    type MetadataQuery: MetadataQuery<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn metadata_query(&self) -> &Self::MetadataQuery;
}

pub trait MetadataEventQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        metadata_id: &MetadataId,
        since: Option<&EventVersion<Metadata>>,
    ) -> error_stack::Result<Vec<EventEnvelope<MetadataEvent, Metadata>>, KernelError>;
}

pub trait DependOnMetadataEventQuery: Sync + Send {
    type MetadataEventQuery: MetadataEventQuery<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn metadata_event_query(&self) -> &Self::MetadataEventQuery;
}
