use crate::transfer::metadata::MetadataDto;
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use adapter::processor::metadata::{
    DependOnMetadataCommandProcessor, DependOnMetadataQueryProcessor, MetadataCommandProcessor,
    MetadataQueryProcessor,
};
use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{DependOnMetadataEventStore, MetadataEventStore};
use kernel::interfaces::read_model::{DependOnMetadataReadModel, MetadataReadModel};
use kernel::prelude::entity::{
    Account, AuthAccountId, EventId, Metadata, MetadataContent, MetadataId, MetadataLabel, Nanoid,
};
use kernel::KernelError;
use std::future::Future;

pub trait UpdateMetadata:
    'static + DependOnDatabaseConnection + DependOnMetadataReadModel + DependOnMetadataEventStore
{
    fn update_metadata(
        &self,
        metadata_id: MetadataId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;
            let existing = self
                .metadata_read_model()
                .find_by_id(&mut transaction, &metadata_id)
                .await?;
            let event_id = EventId::from(metadata_id.clone());

            if let Some(metadata) = existing {
                let events = self
                    .metadata_event_store()
                    .find_by_id(&mut transaction, &event_id, Some(metadata.version()))
                    .await?;
                if events
                    .last()
                    .map(|event| &event.version != metadata.version())
                    .unwrap_or(false)
                {
                    let mut metadata = Some(metadata);
                    for event in events {
                        Metadata::apply(&mut metadata, event)?;
                    }
                    if let Some(metadata) = metadata {
                        self.metadata_read_model()
                            .update(&mut transaction, &metadata)
                            .await?;
                    } else {
                        self.metadata_read_model()
                            .delete(&mut transaction, &metadata_id)
                            .await?;
                    }
                }
            } else {
                let events = self
                    .metadata_event_store()
                    .find_by_id(&mut transaction, &event_id, None)
                    .await?;
                if !events.is_empty() {
                    let mut metadata = None;
                    for event in events {
                        Metadata::apply(&mut metadata, event)?;
                    }
                    if let Some(metadata) = metadata {
                        self.metadata_read_model()
                            .create(&mut transaction, &metadata)
                            .await?;
                    }
                }
            }
            Ok(())
        }
    }
}

impl<T> UpdateMetadata for T where
    T: 'static
        + DependOnDatabaseConnection
        + DependOnMetadataReadModel
        + DependOnMetadataEventStore
{
}

pub trait GetMetadataUseCase:
    'static + Sync + Send + DependOnMetadataQueryProcessor + DependOnAccountQueryProcessor
{
    fn get_metadata(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<Vec<MetadataDto>, KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoid = kernel::prelude::entity::Nanoid::<Account>::new(account_nanoid);
            let account = self
                .account_query_processor()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            let accounts = self
                .account_query_processor()
                .find_by_auth_id(&mut transaction, auth_account_id)
                .await?;

            let found = accounts.iter().any(|a| a.id() == account.id());
            if !found {
                return Err(Report::new(KernelError::PermissionDenied)
                    .attach_printable("This account does not belong to the authenticated user"));
            }

            let metadata_list = self
                .metadata_query_processor()
                .find_by_account_id(&mut transaction, account.id())
                .await?;

            Ok(metadata_list.into_iter().map(MetadataDto::from).collect())
        }
    }
}

impl<T> GetMetadataUseCase for T where
    T: 'static + Sync + Send + DependOnMetadataQueryProcessor + DependOnAccountQueryProcessor
{
}

pub trait CreateMetadataUseCase:
    'static + Sync + Send + DependOnMetadataCommandProcessor + DependOnAccountQueryProcessor
{
    fn create_metadata(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoid: String,
        label: String,
        content: String,
    ) -> impl Future<Output = error_stack::Result<MetadataDto, KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoid = kernel::prelude::entity::Nanoid::<Account>::new(account_nanoid);
            let account = self
                .account_query_processor()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            let accounts = self
                .account_query_processor()
                .find_by_auth_id(&mut transaction, auth_account_id)
                .await?;

            let found = accounts.iter().any(|a| a.id() == account.id());
            if !found {
                return Err(Report::new(KernelError::PermissionDenied)
                    .attach_printable("This account does not belong to the authenticated user"));
            }

            let account_id = account.id().clone();
            let metadata_nanoid = Nanoid::<Metadata>::default();
            let metadata = self
                .metadata_command_processor()
                .create(
                    &mut transaction,
                    account_id,
                    MetadataLabel::new(label),
                    MetadataContent::new(content),
                    metadata_nanoid,
                )
                .await?;

            Ok(MetadataDto::from(metadata))
        }
    }
}

impl<T> CreateMetadataUseCase for T where
    T: 'static + Sync + Send + DependOnMetadataCommandProcessor + DependOnAccountQueryProcessor
{
}

pub trait EditMetadataUseCase:
    'static
    + Sync
    + Send
    + DependOnMetadataCommandProcessor
    + DependOnMetadataQueryProcessor
    + DependOnAccountQueryProcessor
{
    fn edit_metadata(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoid: String,
        metadata_nanoid: String,
        label: String,
        content: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoid = kernel::prelude::entity::Nanoid::<Account>::new(account_nanoid);
            let account = self
                .account_query_processor()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            let accounts = self
                .account_query_processor()
                .find_by_auth_id(&mut transaction, auth_account_id)
                .await?;

            let found = accounts.iter().any(|a| a.id() == account.id());
            if !found {
                return Err(Report::new(KernelError::PermissionDenied)
                    .attach_printable("This account does not belong to the authenticated user"));
            }

            let metadata_list = self
                .metadata_query_processor()
                .find_by_account_id(&mut transaction, account.id())
                .await?;

            let metadata = metadata_list
                .into_iter()
                .find(|m| m.nanoid().as_ref() == &metadata_nanoid)
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Metadata not found with nanoid: {}",
                        metadata_nanoid
                    ))
                })?;

            let metadata_id = metadata.id().clone();
            let current_version = metadata.version().clone();
            self.metadata_command_processor()
                .update(
                    &mut transaction,
                    metadata_id,
                    MetadataLabel::new(label),
                    MetadataContent::new(content),
                    current_version,
                )
                .await?;

            Ok(())
        }
    }
}

impl<T> EditMetadataUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnMetadataCommandProcessor
        + DependOnMetadataQueryProcessor
        + DependOnAccountQueryProcessor
{
}

pub trait DeleteMetadataUseCase:
    'static
    + Sync
    + Send
    + DependOnMetadataCommandProcessor
    + DependOnMetadataQueryProcessor
    + DependOnAccountQueryProcessor
{
    fn delete_metadata(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoid: String,
        metadata_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoid = kernel::prelude::entity::Nanoid::<Account>::new(account_nanoid);
            let account = self
                .account_query_processor()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            let accounts = self
                .account_query_processor()
                .find_by_auth_id(&mut transaction, auth_account_id)
                .await?;

            let found = accounts.iter().any(|a| a.id() == account.id());
            if !found {
                return Err(Report::new(KernelError::PermissionDenied)
                    .attach_printable("This account does not belong to the authenticated user"));
            }

            let metadata_list = self
                .metadata_query_processor()
                .find_by_account_id(&mut transaction, account.id())
                .await?;

            let metadata = metadata_list
                .into_iter()
                .find(|m| m.nanoid().as_ref() == &metadata_nanoid)
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Metadata not found with nanoid: {}",
                        metadata_nanoid
                    ))
                })?;

            let metadata_id = metadata.id().clone();
            let current_version = metadata.version().clone();
            self.metadata_command_processor()
                .delete(&mut transaction, metadata_id, current_version)
                .await?;

            Ok(())
        }
    }
}

impl<T> DeleteMetadataUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnMetadataCommandProcessor
        + DependOnMetadataQueryProcessor
        + DependOnAccountQueryProcessor
{
}
