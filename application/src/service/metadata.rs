use crate::permission::{account_edit, account_view, check_permission};
use crate::transfer::metadata::{CreateMetadataDto, MetadataDto, UpdateMetadataDto};
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use adapter::processor::metadata::{
    CreateMetadataParam, DependOnMetadataCommandProcessor, DependOnMetadataQueryProcessor,
    MetadataCommandProcessor, MetadataQueryProcessor, UpdateMetadataParam,
};
use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{DependOnMetadataEventStore, MetadataEventStore};
use kernel::interfaces::permission::DependOnPermissionChecker;
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
            let mut transaction = self.database_connection().get_executor().await?;
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
    'static
    + Sync
    + Send
    + DependOnMetadataQueryProcessor
    + DependOnAccountQueryProcessor
    + DependOnPermissionChecker
{
    fn get_metadata_batch(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoids: Vec<String>,
    ) -> impl Future<Output = error_stack::Result<Vec<MetadataDto>, KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

            let nanoids: Vec<Nanoid<Account>> = account_nanoids
                .into_iter()
                .map(Nanoid::<Account>::new)
                .collect();
            let accounts = self
                .account_query_processor()
                .find_by_nanoids(&mut transaction, &nanoids)
                .await?;

            let mut permitted_accounts = Vec::new();
            for account in accounts {
                if check_permission(self, auth_account_id, &account_view(account.id()))
                    .await
                    .is_ok()
                {
                    permitted_accounts.push(account);
                }
            }

            if permitted_accounts.is_empty() {
                return Ok(Vec::new());
            }

            let account_ids: Vec<_> = permitted_accounts.iter().map(|a| a.id().clone()).collect();
            let nanoid_map: std::collections::HashMap<_, _> = permitted_accounts
                .iter()
                .map(|a| (a.id().clone(), a.nanoid().as_ref().to_string()))
                .collect();

            let metadata_list = self
                .metadata_query_processor()
                .find_by_account_ids(&mut transaction, &account_ids)
                .await?;

            Ok(metadata_list
                .into_iter()
                .filter_map(|metadata| {
                    let account_nanoid = nanoid_map.get(metadata.account_id())?.clone();
                    Some(MetadataDto::new(metadata, account_nanoid))
                })
                .collect())
        }
    }
}

impl<T> GetMetadataUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnMetadataQueryProcessor
        + DependOnAccountQueryProcessor
        + DependOnPermissionChecker
{
}

pub trait CreateMetadataUseCase:
    'static
    + Sync
    + Send
    + DependOnMetadataCommandProcessor
    + DependOnAccountQueryProcessor
    + DependOnPermissionChecker
{
    fn create_metadata(
        &self,
        auth_account_id: &AuthAccountId,
        dto: CreateMetadataDto,
    ) -> impl Future<Output = error_stack::Result<MetadataDto, KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

            let nanoid = kernel::prelude::entity::Nanoid::<Account>::new(dto.account_nanoid);
            let account = self
                .account_query_processor()
                .find_by_nanoid_unfiltered(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &account_edit(account.id())).await?;

            if !account.status().is_active() {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("Cannot create metadata for a suspended or banned account"));
            }

            let account_nanoid_str = account.nanoid().as_ref().to_string();
            let account_id = account.id().clone();
            let metadata_nanoid = Nanoid::<Metadata>::default();
            let metadata = self
                .metadata_command_processor()
                .create(
                    &mut transaction,
                    CreateMetadataParam {
                        account_id,
                        label: MetadataLabel::new(dto.label),
                        content: MetadataContent::new(dto.content),
                        nano_id: metadata_nanoid,
                    },
                )
                .await?;

            Ok(MetadataDto::new(metadata, account_nanoid_str))
        }
    }
}

impl<T> CreateMetadataUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnMetadataCommandProcessor
        + DependOnAccountQueryProcessor
        + DependOnPermissionChecker
{
}

pub trait UpdateMetadataUseCase:
    'static
    + Sync
    + Send
    + DependOnMetadataCommandProcessor
    + DependOnMetadataQueryProcessor
    + DependOnMetadataEventStore
    + DependOnAccountQueryProcessor
    + DependOnPermissionChecker
{
    fn update_metadata(
        &self,
        auth_account_id: &AuthAccountId,
        dto: UpdateMetadataDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

            let nanoid = kernel::prelude::entity::Nanoid::<Account>::new(dto.account_nanoid);
            let account = self
                .account_query_processor()
                .find_by_nanoid_unfiltered(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &account_edit(account.id())).await?;

            if !account.status().is_active() {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("Cannot update metadata for a suspended or banned account"));
            }

            let metadata_list = self
                .metadata_query_processor()
                .find_by_account_id(&mut transaction, account.id())
                .await?;

            let metadata = metadata_list
                .into_iter()
                .find(|m| m.nanoid().as_ref() == &dto.metadata_nanoid)
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Metadata not found with nanoid: {}",
                        dto.metadata_nanoid
                    ))
                })?;

            let metadata_id = metadata.id().clone();
            let (_metadata, current_version) =
                rehydrate_metadata(self, &mut transaction, &metadata_id).await?;
            self.metadata_command_processor()
                .update(
                    &mut transaction,
                    UpdateMetadataParam {
                        metadata_id,
                        label: MetadataLabel::new(dto.label),
                        content: MetadataContent::new(dto.content),
                        current_version,
                    },
                )
                .await?;

            Ok(())
        }
    }
}

impl<T> UpdateMetadataUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnMetadataCommandProcessor
        + DependOnMetadataQueryProcessor
        + DependOnMetadataEventStore
        + DependOnAccountQueryProcessor
        + DependOnPermissionChecker
{
}

pub trait DeleteMetadataUseCase:
    'static
    + Sync
    + Send
    + DependOnMetadataCommandProcessor
    + DependOnMetadataQueryProcessor
    + DependOnAccountQueryProcessor
    + DependOnMetadataEventStore
    + DependOnPermissionChecker
{
    fn delete_metadata(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoid: String,
        metadata_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

            let nanoid = kernel::prelude::entity::Nanoid::<Account>::new(account_nanoid);
            let account = self
                .account_query_processor()
                .find_by_nanoid_unfiltered(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &account_edit(account.id())).await?;

            if !account.status().is_active() {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("Cannot delete metadata for a suspended or banned account"));
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
            let (_metadata, current_version) =
                rehydrate_metadata(self, &mut transaction, &metadata_id).await?;
            self.metadata_command_processor()
                .delete(&mut transaction, metadata_id, current_version)
                .await?;

            Ok(())
        }
    }
}

impl<T> DeleteMetadataUseCase for T where
    T: 'static
        + DependOnMetadataCommandProcessor
        + DependOnMetadataQueryProcessor
        + DependOnAccountQueryProcessor
        + DependOnMetadataEventStore
        + DependOnPermissionChecker
{
}

async fn rehydrate_metadata<T>(
    deps: &T,
    executor: &mut <<T as kernel::interfaces::database::DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    metadata_id: &MetadataId,
) -> error_stack::Result<(Metadata, kernel::prelude::entity::EventVersion<Metadata>), KernelError>
where
    T: DependOnMetadataEventStore + ?Sized,
{
    let event_id = EventId::from(metadata_id.clone());
    let events = deps
        .metadata_event_store()
        .find_by_id(executor, &event_id, None)
        .await?;
    if events.is_empty() {
        return Err(Report::new(KernelError::NotFound).attach_printable(format!(
            "No events found for metadata: {}",
            metadata_id.as_ref()
        )));
    }
    let mut metadata: Option<Metadata> = None;
    for event in events {
        Metadata::apply(&mut metadata, event)?;
    }
    let metadata = metadata.ok_or_else(|| {
        Report::new(KernelError::NotFound).attach_printable(format!(
            "Metadata aggregate could not be reconstructed (already deleted?): {}",
            metadata_id.as_ref()
        ))
    })?;
    let current_version = metadata.version().clone();
    Ok((metadata, current_version))
}
