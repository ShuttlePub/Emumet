use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{DependOnMetadataEventStore, MetadataEventStore};
use kernel::interfaces::read_model::{DependOnMetadataReadModel, MetadataReadModel};
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::{
    AccountId, EventVersion, Metadata, MetadataContent, MetadataId, MetadataLabel, Nanoid,
};
use kernel::KernelError;
use std::future::Future;

// --- Signal DI trait (adapter-specific) ---

pub trait DependOnMetadataSignal: Send + Sync {
    type MetadataSignal: Signal<MetadataId> + Send + Sync + 'static;
    fn metadata_signal(&self) -> &Self::MetadataSignal;
}

// --- MetadataCommandProcessor ---

pub trait MetadataCommandProcessor: Send + Sync + 'static {
    type Executor: Executor;

    fn create(
        &self,
        executor: &mut Self::Executor,
        account_id: AccountId,
        label: MetadataLabel,
        content: MetadataContent,
        nano_id: Nanoid<Metadata>,
    ) -> impl Future<Output = error_stack::Result<Metadata, KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Executor,
        metadata_id: MetadataId,
        label: MetadataLabel,
        content: MetadataContent,
        current_version: EventVersion<Metadata>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Executor,
        metadata_id: MetadataId,
        current_version: EventVersion<Metadata>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

impl<T> MetadataCommandProcessor for T
where
    T: DependOnMetadataEventStore + DependOnMetadataSignal + Send + Sync + 'static,
{
    type Executor =
        <<T as DependOnMetadataEventStore>::MetadataEventStore as MetadataEventStore>::Executor;

    async fn create(
        &self,
        executor: &mut Self::Executor,
        account_id: AccountId,
        label: MetadataLabel,
        content: MetadataContent,
        nano_id: Nanoid<Metadata>,
    ) -> error_stack::Result<Metadata, KernelError> {
        let metadata_id = MetadataId::new(uuid::Uuid::now_v7());
        let command = Metadata::create(metadata_id.clone(), account_id, label, content, nano_id);

        let event_envelope = self
            .metadata_event_store()
            .persist_and_transform(executor, command)
            .await?;

        let mut metadata = None;
        Metadata::apply(&mut metadata, event_envelope)?;
        let metadata = metadata.ok_or_else(|| {
            Report::new(KernelError::Internal)
                .attach_printable("Failed to construct metadata from created event")
        })?;

        if let Err(e) = self.metadata_signal().emit(metadata_id).await {
            tracing::warn!("Failed to emit metadata signal: {:?}", e);
        }

        Ok(metadata)
    }

    async fn update(
        &self,
        executor: &mut Self::Executor,
        metadata_id: MetadataId,
        label: MetadataLabel,
        content: MetadataContent,
        current_version: EventVersion<Metadata>,
    ) -> error_stack::Result<(), KernelError> {
        let command = Metadata::update(metadata_id.clone(), label, content, current_version);

        self.metadata_event_store()
            .persist_and_transform(executor, command)
            .await?;

        if let Err(e) = self.metadata_signal().emit(metadata_id).await {
            tracing::warn!("Failed to emit metadata signal: {:?}", e);
        }

        Ok(())
    }

    async fn delete(
        &self,
        executor: &mut Self::Executor,
        metadata_id: MetadataId,
        current_version: EventVersion<Metadata>,
    ) -> error_stack::Result<(), KernelError> {
        let command = Metadata::delete(metadata_id.clone(), current_version);

        self.metadata_event_store()
            .persist_and_transform(executor, command)
            .await?;

        if let Err(e) = self.metadata_signal().emit(metadata_id).await {
            tracing::warn!("Failed to emit metadata signal: {:?}", e);
        }

        Ok(())
    }
}

pub trait DependOnMetadataCommandProcessor: DependOnDatabaseConnection + Send + Sync {
    type MetadataCommandProcessor: MetadataCommandProcessor<
        Executor = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    >;
    fn metadata_command_processor(&self) -> &Self::MetadataCommandProcessor;
}

impl<T> DependOnMetadataCommandProcessor for T
where
    T: DependOnMetadataEventStore
        + DependOnMetadataSignal
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

// --- MetadataQueryProcessor ---

pub trait MetadataQueryProcessor: Send + Sync + 'static {
    type Executor: Executor;

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
}

impl<T> MetadataQueryProcessor for T
where
    T: DependOnMetadataReadModel + Send + Sync + 'static,
{
    type Executor =
        <<T as DependOnMetadataReadModel>::MetadataReadModel as MetadataReadModel>::Executor;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &MetadataId,
    ) -> error_stack::Result<Option<Metadata>, KernelError> {
        self.metadata_read_model().find_by_id(executor, id).await
    }

    async fn find_by_account_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> error_stack::Result<Vec<Metadata>, KernelError> {
        self.metadata_read_model()
            .find_by_account_id(executor, account_id)
            .await
    }
}

pub trait DependOnMetadataQueryProcessor: DependOnDatabaseConnection + Send + Sync {
    type MetadataQueryProcessor: MetadataQueryProcessor<
        Executor = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
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
