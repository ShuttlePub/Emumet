use kernel::interfaces::database::{
    DependOnDatabaseConnection, Savepoint, Transaction, TransactionalDatabaseConnection,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::projection::{
    DependOnMetadataEventLog, DependOnMetadataProjectionWriter, DependOnProjectionCheckpointStore,
    MetadataEventLog, MetadataProjectionWriter, ProjectionCheckpointStore,
};
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnMetadataReadModel, MetadataReadModel,
};
use kernel::prelude::entity::{EventEnvelope, Metadata, MetadataEvent, MetadataId};
use kernel::KernelError;
use std::collections::HashMap;
use std::future::Future;

pub const METADATA_PROJECTOR_NAME: &str = "metadata_projector";

pub const METADATA_PROJECTOR_WINDOW: i64 = 100;

pub const METADATA_PROJECTOR_BATCH_LIMIT: i64 = 1000;

pub trait ProjectMetadataBatch:
    DependOnDatabaseConnection<DatabaseConnection: TransactionalDatabaseConnection>
    + DependOnAccountReadModel
    + DependOnMetadataEventLog
    + DependOnMetadataReadModel
    + DependOnProjectionCheckpointStore
    + DependOnMetadataProjectionWriter
{
    fn project_metadata_batch(
        &self,
    ) -> impl Future<Output = error_stack::Result<i64, KernelError>> + Send + '_ {
        async move {
            let mut transaction = self.database_connection().get_transaction().await?;

            let checkpoint = {
                let executor = transaction.connection();
                self.projection_checkpoint_store()
                    .get(executor, METADATA_PROJECTOR_NAME)
                    .await?
                    .unwrap_or(0)
            };
            let events = {
                let executor = transaction.connection();
                self.metadata_event_log()
                    .find_by_seq_window(
                        executor,
                        checkpoint - METADATA_PROJECTOR_WINDOW,
                        METADATA_PROJECTOR_BATCH_LIMIT,
                    )
                    .await?
            };
            if events.is_empty() {
                transaction.commit().await?;
                return Ok(checkpoint);
            }
            let max_seq = events.last().map(|event| event.seq).unwrap_or(checkpoint);

            let mut groups: HashMap<MetadataId, Vec<EventEnvelope<MetadataEvent, Metadata>>> =
                HashMap::new();
            for event in events {
                groups
                    .entry(MetadataId::new(*event.envelope.id.as_ref()))
                    .or_default()
                    .push(event.envelope);
            }

            for (metadata_id, mut envelopes) in groups {
                envelopes.sort_by_key(|event| *event.version.as_ref());

                let existing = {
                    let executor = transaction.connection();
                    self.metadata_read_model()
                        .find_by_id_unfiltered(executor, &metadata_id)
                        .await?
                };
                let pending: Vec<_> = envelopes
                    .into_iter()
                    .filter(|event| match &existing {
                        Some(metadata) => *event.version.as_ref() > *metadata.version().as_ref(),
                        None => true,
                    })
                    .collect();
                if pending.is_empty() {
                    continue;
                }

                // On the fresh-materialization path (existing is None), check that
                // the parent account has not been cascade-deleted.  If the account
                // has been deleted (deleted_at present), skip upsert so the
                // projector does not resurrect rows that the Account projector
                // intentionally removed.
                if existing.is_none() {
                    let account_id = pending.iter().find_map(|event| match &event.event {
                        MetadataEvent::Created { account_id, .. } => Some(account_id.clone()),
                        _ => None,
                    });
                    if let Some(aid) = account_id {
                        let executor = transaction.connection();
                        if let Ok(Some(account)) = self
                            .account_read_model()
                            .find_by_id_including_deleted(executor, &aid)
                            .await
                        {
                            if account.deleted_at().is_some() {
                                tracing::debug!(
                                    account_id = %aid.as_ref(),
                                    "metadata projector: skipping upsert for metadata {} (parent account deleted)",
                                    metadata_id.as_ref()
                                );
                                continue;
                            }
                        }
                    }
                }

                let mut entity = existing.map(Metadata::from);
                let mut fold_failed = false;
                for event in pending {
                    if let Err(error) = Metadata::apply(&mut entity, event) {
                        tracing::warn!(
                            ?error,
                            metadata_id = %metadata_id.as_ref(),
                            "metadata projection fold skipped (incomplete stream); window re-read will retry"
                        );
                        fold_failed = true;
                        break;
                    }
                }
                if fold_failed {
                    continue;
                }

                let metadata_id_for_write = metadata_id.clone();
                let savepoint = transaction.savepoint().await?;
                let write_result: error_stack::Result<(), KernelError> = {
                    let executor = transaction.connection();
                    async {
                        match entity {
                            Some(metadata) => {
                                self.metadata_projection_writer()
                                    .upsert(executor, &metadata)
                                    .await?;
                            }
                            None => {
                                self.metadata_projection_writer()
                                    .delete(executor, &metadata_id_for_write)
                                    .await?;
                            }
                        }
                        Ok(())
                    }
                    .await
                };
                match write_result {
                    Ok(()) => {
                        let executor = transaction.connection();
                        savepoint.commit(executor).await?;
                    }
                    Err(error) => {
                        let executor = transaction.connection();
                        savepoint.rollback(executor).await?;
                        tracing::warn!(
                            ?error,
                            metadata_id = %metadata_id.as_ref(),
                            "metadata projection skipped"
                        );
                    }
                }
            }

            let executor = transaction.connection();
            self.projection_checkpoint_store()
                .set(executor, METADATA_PROJECTOR_NAME, max_seq)
                .await?;
            transaction.commit().await?;
            Ok(max_seq)
        }
    }
}

impl<T> ProjectMetadataBatch for T where
    T: DependOnDatabaseConnection<DatabaseConnection: TransactionalDatabaseConnection>
        + DependOnAccountReadModel
        + DependOnMetadataEventLog
        + DependOnMetadataReadModel
        + DependOnProjectionCheckpointStore
        + DependOnMetadataProjectionWriter
{
}
