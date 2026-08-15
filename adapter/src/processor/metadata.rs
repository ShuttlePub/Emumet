use error_stack::Report;
use kernel::interfaces::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{
    DependOnMetadataReadModel, MetadataProjection, MetadataReadModel,
};
use kernel::interfaces::repository::{AggregateRepository, DependOnMetadataRepository};
use kernel::prelude::entity::{
    AccountId, EventVersion, Metadata, MetadataContent, MetadataId, MetadataLabel, Nanoid,
};
use kernel::KernelError;
use std::future::Future;

#[derive(Debug)]
pub struct CreateMetadataParam {
    pub account_id: AccountId,
    pub label: MetadataLabel,
    pub content: MetadataContent,
    pub nano_id: Nanoid<Metadata>,
}

#[derive(Debug)]
pub struct UpdateMetadataParam {
    pub metadata_id: MetadataId,
    pub label: MetadataLabel,
    pub content: MetadataContent,
    pub current_version: EventVersion<Metadata>,
}

pub trait MetadataCommandProcessor: Send + Sync + 'static {
    type Connection: Connection;

    fn create(
        &self,
        executor: &mut Self::Connection,
        param: CreateMetadataParam,
    ) -> impl Future<Output = error_stack::Result<Metadata, KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Connection,
        param: UpdateMetadataParam,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Connection,
        metadata_id: MetadataId,
        current_version: EventVersion<Metadata>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

impl<T> MetadataCommandProcessor for T
where
    T: DependOnMetadataRepository + DependOnMetadataReadModel + Send + Sync + 'static,
{
    type Connection =
        <<T as DependOnMetadataRepository>::MetadataRepository as AggregateRepository<Metadata>>::Connection;

    async fn create(
        &self,
        executor: &mut Self::Connection,
        param: CreateMetadataParam,
    ) -> error_stack::Result<Metadata, KernelError> {
        let metadata_id = MetadataId::new(kernel::generate_id());
        let command = Metadata::create(
            metadata_id.clone(),
            param.account_id,
            param.label,
            param.content,
            param.nano_id,
        );

        let event_envelope = self.metadata_repository().save(executor, command).await?;

        let mut metadata = None;
        Metadata::apply(&mut metadata, event_envelope)?;
        let metadata = metadata.ok_or_else(|| {
            Report::new(KernelError::Internal)
                .attach_printable("Failed to construct metadata from created event")
        })?;

        if let Err(e) = self.metadata_read_model().create(executor, &metadata).await {
            tracing::error!(?e, "Failed to create metadata read model");
            return Err(e);
        }

        Ok(metadata)
    }

    async fn update(
        &self,
        executor: &mut Self::Connection,
        param: UpdateMetadataParam,
    ) -> error_stack::Result<(), KernelError> {
        let command = Metadata::update(
            param.metadata_id.clone(),
            param.label,
            param.content,
            param.current_version,
        );

        self.metadata_repository().save(executor, command).await?;
        Ok(())
    }

    async fn delete(
        &self,
        executor: &mut Self::Connection,
        metadata_id: MetadataId,
        current_version: EventVersion<Metadata>,
    ) -> error_stack::Result<(), KernelError> {
        let command = Metadata::delete(metadata_id, current_version);
        self.metadata_repository().save(executor, command).await?;
        Ok(())
    }
}

pub trait DependOnMetadataCommandProcessor: DependOnDatabaseConnection + Send + Sync {
    type MetadataCommandProcessor: MetadataCommandProcessor<
        Connection = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    >;
    fn metadata_command_processor(&self) -> &Self::MetadataCommandProcessor;
}

impl<T> DependOnMetadataCommandProcessor for T
where
    T: DependOnMetadataRepository
        + DependOnMetadataReadModel
        + DependOnDatabaseConnection
        + Send
        + Sync
        + 'static,
{
    type MetadataCommandProcessor = Self;
    fn metadata_command_processor(&self) -> &Self::MetadataCommandProcessor {
        self
    }
}

pub trait MetadataQueryProcessor: Send + Sync + 'static {
    type Connection: Connection;

    fn find_by_id(
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
}

impl<T> MetadataQueryProcessor for T
where
    T: DependOnMetadataReadModel + Send + Sync + 'static,
{
    type Connection =
        <<T as DependOnMetadataReadModel>::MetadataReadModel as MetadataReadModel>::Connection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &MetadataId,
    ) -> error_stack::Result<Option<MetadataProjection>, KernelError> {
        self.metadata_read_model().find_by_id(executor, id).await
    }

    async fn find_by_account_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> error_stack::Result<Vec<MetadataProjection>, KernelError> {
        self.metadata_read_model()
            .find_by_account_id(executor, account_id)
            .await
    }

    async fn find_by_account_ids(
        &self,
        executor: &mut Self::Connection,
        account_ids: &[AccountId],
    ) -> error_stack::Result<Vec<MetadataProjection>, KernelError> {
        self.metadata_read_model()
            .find_by_account_ids(executor, account_ids)
            .await
    }
}

pub trait DependOnMetadataQueryProcessor: DependOnDatabaseConnection + Send + Sync {
    type MetadataQueryProcessor: MetadataQueryProcessor<
        Connection = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    >;
    fn metadata_query_processor(&self) -> &Self::MetadataQueryProcessor;
}

impl<T> DependOnMetadataQueryProcessor for T
where
    T: DependOnMetadataReadModel + DependOnDatabaseConnection + Send + Sync + 'static,
{
    type MetadataQueryProcessor = Self;
    fn metadata_query_processor(&self) -> &Self::MetadataQueryProcessor {
        self
    }
}
