use kernel::interfaces::database::{
    DependOnDatabaseConnection, Savepoint, Transaction, TransactionalDatabaseConnection,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::home_link::{DependOnHomeLinkStore, HomeLinkStore};
use kernel::interfaces::projection::{
    DependOnProfileEventLog, DependOnProfileProjectionWriter, DependOnProjectionCheckpointStore,
    ProfileEventLog, ProfileProjectionWriter, ProjectionCheckpointStore,
};
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::prelude::entity::{AccountId, EventEnvelope, Profile, ProfileEvent, ProfileId};
use kernel::KernelError;
use std::collections::HashMap;
use std::future::Future;

pub const PROFILE_PROJECTOR_NAME: &str = "profile_projector";

pub const PROFILE_PROJECTOR_WINDOW: i64 = 100;

pub const PROFILE_PROJECTOR_BATCH_LIMIT: i64 = 1000;

/// What one Profile group needs after folding its pending events.
pub enum GroupPlan {
    /// Nothing to write (already projected, skipped, or an incomplete stream).
    Skip,
    /// Write `entity` (`None` = delete). `owners` are the Accounts whose Profile set the write
    /// changes: empty for an in-place update of an existing row of the same Account.
    Write {
        entity: Option<Profile>,
        owners: Vec<AccountId>,
    },
}

pub trait ProjectProfileBatch:
    DependOnDatabaseConnection<DatabaseConnection: TransactionalDatabaseConnection>
    + DependOnAccountReadModel
    + DependOnProfileEventLog
    + DependOnProfileReadModel
    + DependOnProjectionCheckpointStore
    + DependOnProfileProjectionWriter
    + DependOnHomeLinkStore
{
    /// Folds one Profile group against the current read model without writing.
    fn plan_profile_group<'a>(
        &'a self,
        transaction: &'a mut <Self::DatabaseConnection as TransactionalDatabaseConnection>::Transaction,
        profile_id: &'a ProfileId,
        mut envelopes: Vec<EventEnvelope<ProfileEvent, Profile>>,
    ) -> impl Future<Output = error_stack::Result<GroupPlan, KernelError>> + Send + 'a {
        async move {
            envelopes.sort_by_key(|event| *event.version.as_ref());

            let existing = {
                let executor = transaction.connection();
                self.profile_read_model()
                    .find_by_id_unfiltered(executor, profile_id)
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
                return Ok(GroupPlan::Skip);
            }

            // On the fresh-materialization path (existing is None), check that
            // the parent account has not been cascade-deleted.  If the account
            // has been deleted (deleted_at present), skip upsert so the
            // projector does not resurrect rows that the Account projector
            // intentionally removed.
            if existing.is_none() {
                let account_id = pending.iter().find_map(|event| match &event.event {
                    ProfileEvent::Created { account_id, .. } => Some(account_id.clone()),
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
                                "profile projector: skipping upsert for profile {} (parent account deleted)",
                                profile_id.as_ref()
                            );
                            return Ok(GroupPlan::Skip);
                        }
                    }
                }
            }

            let previous_owner = existing
                .as_ref()
                .map(|profile| profile.account_id().clone());
            let mut entity = existing.map(Profile::from);
            for event in pending {
                if let Err(error) = Profile::apply(&mut entity, event) {
                    tracing::warn!(
                        ?error,
                        profile_id = %profile_id.as_ref(),
                        "profile projection fold skipped (incomplete stream); window re-read will retry"
                    );
                    return Ok(GroupPlan::Skip);
                }
            }
            // Only a write that adds a Profile to, removes it from, or moves it between Accounts
            // changes an Account's Profile set. Those run under the Account gate(s).
            let new_owner = entity.as_ref().map(|profile| profile.account_id().clone());
            let mut owners = Vec::new();
            if previous_owner != new_owner || previous_owner.is_none() {
                owners.extend(previous_owner);
                owners.extend(new_owner);
                owners.sort_by_key(|id| *id.as_ref());
                owners.dedup();
            }
            Ok(GroupPlan::Write { entity, owners })
        }
    }

    /// Writes a planned group inside a savepoint; a failing write is logged and skipped (the
    /// window re-read retries), exactly as before.
    fn write_profile_group<'a>(
        &'a self,
        transaction: &'a mut <Self::DatabaseConnection as TransactionalDatabaseConnection>::Transaction,
        profile_id: &'a ProfileId,
        entity: Option<Profile>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
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
                                .delete(executor, profile_id)
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
            Ok(())
        }
    }

    /// One Profile whose write changes an Account's Profile set, in its own bounded transaction:
    /// Account gate(s) ascending -> Profile gate -> (re-plan) -> write. The plan is made once to
    /// learn the owners, the gates are taken, and the plan is made again; if the owners changed in
    /// between, the gate set was incomplete and the attempt fails with `Concurrency` so the caller
    /// retries in a new transaction instead of taking a gate out of order.
    fn project_gated_profile_group<'a>(
        &'a self,
        profile_id: &'a ProfileId,
        envelopes: &'a [EventEnvelope<ProfileEvent, Profile>],
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            let mut attempt = 0u8;
            loop {
                let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    let mut transaction = self.database_connection().get_transaction().await?;
                    let first = self
                        .plan_profile_group(&mut transaction, profile_id, envelopes.to_vec())
                        .await?;
                    let GroupPlan::Write { owners, .. } = first else {
                        transaction.commit().await?;
                        return Ok(());
                    };
                    self.home_link_store()
                        .lock_accounts(transaction.connection(), &owners)
                        .await?;
                    self.home_link_store()
                        .lock_profiles(transaction.connection(), std::slice::from_ref(profile_id))
                        .await?;
                    let second = self
                        .plan_profile_group(&mut transaction, profile_id, envelopes.to_vec())
                        .await?;
                    if let GroupPlan::Write {
                        entity,
                        owners: locked_for,
                    } = second
                    {
                        if locked_for.iter().any(|owner| !owners.contains(owner)) {
                            return Err(error_stack::Report::new(KernelError::Concurrency)
                                .attach_printable("Profile owner changed while taking its gates"));
                        }
                        self.write_profile_group(&mut transaction, profile_id, entity)
                            .await?;
                    }
                    transaction.commit().await
                })
                .await
                .map_err(|_| {
                    error_stack::Report::new(KernelError::Internal)
                        .attach_printable("Profile projection lifecycle transaction exceeded 5s")
                })?;
                match outcome {
                    Err(error)
                        if matches!(error.current_context(), KernelError::Concurrency)
                            && attempt < 2 =>
                    {
                        attempt += 1;
                    }
                    other => return other,
                }
            }
        }
    }

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

            // In-place updates of an existing row (the common case) stay in this batch
            // transaction. A write that changes an Account's Profile set is deferred to its own
            // Account-gated transaction below: the set only changes under the Account's gate.
            let mut deferred: Vec<(ProfileId, Vec<EventEnvelope<ProfileEvent, Profile>>)> =
                Vec::new();
            for (profile_id, envelopes) in groups {
                match self
                    .plan_profile_group(&mut transaction, &profile_id, envelopes.clone())
                    .await?
                {
                    GroupPlan::Skip => {}
                    GroupPlan::Write { owners, .. } if !owners.is_empty() => {
                        deferred.push((profile_id, envelopes));
                    }
                    GroupPlan::Write { entity, .. } => {
                        self.write_profile_group(&mut transaction, &profile_id, entity)
                            .await?;
                    }
                }
            }

            if deferred.is_empty() {
                let executor = transaction.connection();
                self.projection_checkpoint_store()
                    .set(executor, PROFILE_PROJECTOR_NAME, max_seq)
                    .await?;
                transaction.commit().await?;
                return Ok(max_seq);
            }
            transaction.commit().await?;
            deferred.sort_by_key(|(profile_id, _)| *profile_id.as_ref());
            for (profile_id, envelopes) in &deferred {
                self.project_gated_profile_group(profile_id, envelopes)
                    .await?;
            }
            let mut transaction = self.database_connection().get_transaction().await?;
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
        + DependOnAccountReadModel
        + DependOnProfileEventLog
        + DependOnProfileReadModel
        + DependOnProjectionCheckpointStore
        + DependOnProfileProjectionWriter
        + DependOnHomeLinkStore
{
}
