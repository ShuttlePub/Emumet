use kernel::interfaces::database::{
    DependOnDatabaseConnection, Savepoint, Transaction, TransactionalDatabaseConnection,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::projection::{
    DependOnProfileEventLog, DependOnProfileProjectionWriter, DependOnProjectionCheckpointStore,
    ProfileEventLog, ProfileProjectionWriter, ProjectionCheckpointStore,
};
use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
use kernel::prelude::entity::{EventEnvelope, Profile, ProfileEvent, ProfileId};
use kernel::KernelError;
use std::collections::HashMap;
use std::future::Future;

pub const PROFILE_PROJECTOR_NAME: &str = "profile_projector";

pub const PROFILE_PROJECTOR_WINDOW: i64 = 100;

pub const PROFILE_PROJECTOR_BATCH_LIMIT: i64 = 1000;

pub trait ProjectProfileBatch:
    DependOnDatabaseConnection<DatabaseConnection: TransactionalDatabaseConnection>
    + DependOnProfileEventLog
    + DependOnProfileReadModel
    + DependOnProjectionCheckpointStore
    + DependOnProfileProjectionWriter
{
    fn project_profile_batch(
        &self,
    ) -> impl Future<Output = error_stack::Result<i64, KernelError>> + Send + '_ {
        async move {
            let mut transaction = self.database_connection().get_transaction().await?;

            let checkpoint = {
                let executor = transaction.connection();
                self.projection_checkpoint_store()
                    .get(executor, PROFILE_PROJECTOR_NAME)
                    .await?
                    .unwrap_or(0)
            };
            let events = {
                let executor = transaction.connection();
                self.profile_event_log()
                    .find_by_seq_window(
                        executor,
                        checkpoint - PROFILE_PROJECTOR_WINDOW,
                        PROFILE_PROJECTOR_BATCH_LIMIT,
                    )
                    .await?
            };
            if events.is_empty() {
                transaction.commit().await?;
                return Ok(checkpoint);
            }
            let max_seq = events.last().map(|event| event.seq).unwrap_or(checkpoint);

            let mut groups: HashMap<ProfileId, Vec<EventEnvelope<ProfileEvent, Profile>>> =
                HashMap::new();
            for event in events {
                groups
                    .entry(ProfileId::new(*event.envelope.id.as_ref()))
                    .or_default()
                    .push(event.envelope);
            }

            for (profile_id, mut envelopes) in groups {
                envelopes.sort_by_key(|event| *event.version.as_ref());

                let existing = {
                    let executor = transaction.connection();
                    self.profile_read_model()
                        .find_by_id_unfiltered(executor, &profile_id)
                        .await?
                };
                let pending: Vec<_> = envelopes
                    .into_iter()
                    .filter(|event| match &existing {
                        Some(profile) => *event.version.as_ref() > *profile.version().as_ref(),
                        None => true,
                    })
                    .collect();
                if pending.is_empty() {
                    continue;
                }

                let mut entity = existing.map(Profile::from);
                let mut fold_failed = false;
                for event in pending {
                    if let Err(error) = Profile::apply(&mut entity, event) {
                        tracing::warn!(
                            ?error,
                            profile_id = %profile_id.as_ref(),
                            "profile projection fold skipped (incomplete stream); window re-read will retry"
                        );
                        fold_failed = true;
                        break;
                    }
                }
                if fold_failed {
                    continue;
                }

                let profile_id_for_write = profile_id.clone();
                let savepoint = transaction.savepoint().await?;
                let write_result: error_stack::Result<(), KernelError> = {
                    let executor = transaction.connection();
                    async {
                        match entity {
                            Some(profile) => {
                                self.profile_projection_writer()
                                    .upsert(executor, &profile)
                                    .await?;
                            }
                            None => {
                                self.profile_projection_writer()
                                    .delete(executor, &profile_id_for_write)
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
                            profile_id = %profile_id.as_ref(),
                            "profile projection skipped"
                        );
                    }
                }
            }

            let executor = transaction.connection();
            self.projection_checkpoint_store()
                .set(executor, PROFILE_PROJECTOR_NAME, max_seq)
                .await?;
            transaction.commit().await?;
            Ok(max_seq)
        }
    }
}

impl<T> ProjectProfileBatch for T where
    T: DependOnDatabaseConnection<DatabaseConnection: TransactionalDatabaseConnection>
        + DependOnProfileEventLog
        + DependOnProfileReadModel
        + DependOnProjectionCheckpointStore
        + DependOnProfileProjectionWriter
{
}
