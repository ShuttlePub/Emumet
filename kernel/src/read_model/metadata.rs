use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    AccountId, EventVersion, Metadata, MetadataContent, MetadataId, MetadataLabel, Nanoid,
};
use crate::KernelError;
use std::future::Future;

/// Projection DTO for metadata reads (ADR 0006 decision 9).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MetadataProjection {
    id: MetadataId,
    account_id: AccountId,
    label: MetadataLabel,
    content: MetadataContent,
    version: EventVersion<Metadata>,
    nanoid: Nanoid<Metadata>,
}

impl MetadataProjection {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: MetadataId,
        account_id: AccountId,
        label: MetadataLabel,
        content: MetadataContent,
        version: EventVersion<Metadata>,
        nanoid: Nanoid<Metadata>,
    ) -> Self {
        Self {
            id,
            account_id,
            label,
            content,
            version,
            nanoid,
        }
    }

    pub fn id(&self) -> &MetadataId {
        &self.id
    }

    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    pub fn label(&self) -> &MetadataLabel {
        &self.label
    }

    pub fn content(&self) -> &MetadataContent {
        &self.content
    }

    pub fn version(&self) -> &EventVersion<Metadata> {
        &self.version
    }

    pub fn nanoid(&self) -> &Nanoid<Metadata> {
        &self.nanoid
    }
}

impl From<Metadata> for MetadataProjection {
    fn from(value: Metadata) -> Self {
        let destruct = value.into_destruct();
        Self::new(
            destruct.id,
            destruct.account_id,
            destruct.label,
            destruct.content,
            destruct.version,
            destruct.nanoid,
        )
    }
}

impl From<MetadataProjection> for Metadata {
    fn from(value: MetadataProjection) -> Self {
        Metadata::reconstitute(
            value.id().clone(),
            value.account_id().clone(),
            value.label().clone(),
            value.content().clone(),
            value.nanoid().clone(),
            value.version().clone(),
        )
    }
}

pub trait MetadataReadModel: Sync + Send + 'static {
    type Connection: Connection;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &MetadataId,
    ) -> impl Future<Output = error_stack::Result<Option<MetadataProjection>, KernelError>> + Send;

    fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Connection,
        id: &MetadataId,
    ) -> impl Future<Output = error_stack::Result<Option<MetadataProjection>, KernelError>> + Send;

    fn find_by_account_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<MetadataProjection>, KernelError>> + Send;

    fn find_by_account_ids(
        &self,
        executor: &mut Self::Connection,
        account_ids: &[AccountId],
    ) -> impl Future<Output = error_stack::Result<Vec<MetadataProjection>, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Connection,
        metadata: &Metadata,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Connection,
        metadata: &Metadata,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Connection,
        metadata_id: &MetadataId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnMetadataReadModel: Sync + Send + DependOnDatabaseConnection {
    type MetadataReadModel: MetadataReadModel<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn metadata_read_model(&self) -> &Self::MetadataReadModel;
}
