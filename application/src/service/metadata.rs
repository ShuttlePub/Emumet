use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{DependOnMetadataEventStore, MetadataEventStore};
use kernel::interfaces::read_model::{DependOnMetadataReadModel, MetadataReadModel};
use kernel::prelude::entity::{EventId, Metadata, MetadataId};
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

pub(crate) async fn rehydrate_metadata<T>(
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
