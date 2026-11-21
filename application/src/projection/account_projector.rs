use kernel::interfaces::database::{
    DependOnDatabaseConnection, Savepoint, Transaction, TransactionalDatabaseConnection,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::home_link::{DependOnHomeLinkStore, HomeLinkStore};
use kernel::interfaces::permission::{
    AccountRelation, DependOnPermissionWriter, PermissionWriter, RelationTarget,
};
use kernel::interfaces::projection::{
    AccountEventLog, AccountProjectionWriter, DependOnAccountEventLog,
    DependOnAccountProjectionWriter, DependOnProfileEventLog, DependOnProfileProjectionWriter,
    DependOnProjectionCheckpointStore, ProfileEventLog, ProfileProjectionWriter,
    ProjectionCheckpointStore,
};
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnMetadataReadModel,
    DependOnProfileReadModel, MetadataReadModel,
};
use kernel::interfaces::repository::{
    AuthAccountRepository, DependOnAuthAccountRepository, DependOnFollowRepository,
    FollowRepository,
};
use kernel::prelude::entity::{
    Account, AccountEvent, AccountId, AccountKind, EventEnvelope, FollowTargetId, Profile,
    ProfileId,
};
use kernel::KernelError;
use std::collections::HashMap;
use std::future::Future;

pub const ACCOUNT_PROJECTOR_NAME: &str = "account_projector";

/// Window re-read margin `W`: each poll reads `seq > checkpoint - W` so events
/// committed out of seq order are eventually applied. The naive monotonic tail
/// (`seq > checkpoint` only) would permanently miss a late commit.
pub const ACCOUNT_PROJECTOR_WINDOW: i64 = 100;

/// Upper bound of one poll batch. Must exceed `ACCOUNT_PROJECTOR_WINDOW` so a
/// poll always advances: when more than `BATCH_LIMIT` events sit in the window,
/// the lowest `BATCH_LIMIT` still move the checkpoint by at least
/// `BATCH_LIMIT - WINDOW`.
pub const ACCOUNT_PROJECTOR_BATCH_LIMIT: i64 = 1000;

/// Transactional log tailing projector for account projections (ADR 0006
/// decision 4/6). One poll reads the seq window, folds the batch per aggregate
/// in version order, writes the projection through the version-gated writer and
/// advances the checkpoint. Each Account has its own bounded gate-bearing transaction;
/// permission service calls occur after commit and checkpointing is separate.
pub trait ProjectAccountBatch:
    DependOnDatabaseConnection<DatabaseConnection: TransactionalDatabaseConnection>
    + DependOnHomeLinkStore
    + DependOnAuthAccountRepository
    + DependOnAccountEventLog
    + DependOnProjectionCheckpointStore
    + DependOnAccountProjectionWriter
    + DependOnAccountReadModel
    + DependOnProfileReadModel
    + DependOnMetadataReadModel
    + DependOnFollowRepository
    + DependOnPermissionWriter
    + DependOnProfileEventLog
    + DependOnProfileProjectionWriter
{
    /// Run one tailing poll. Returns the checkpoint after the poll.
    fn project_batch(
        &self,
    ) -> impl Future<Output = error_stack::Result<i64, KernelError>> + Send + '_ {
        async move {
            let mut transaction = self.database_connection().get_transaction().await?;

            let checkpoint = {
                let executor = transaction.connection();
                self.projection_checkpoint_store()
                    .get(executor, ACCOUNT_PROJECTOR_NAME)
                    .await?
                    .unwrap_or(0)
            };
            let events = {
                let executor = transaction.connection();
                self.account_event_log()
                    .find_by_seq_window(
                        executor,
                        checkpoint - ACCOUNT_PROJECTOR_WINDOW,
                        ACCOUNT_PROJECTOR_BATCH_LIMIT,
                    )
                    .await?
            };
            if events.is_empty() {
                transaction.commit().await?;
                return Ok(checkpoint);
            }
            let max_seq = events.last().map(|event| event.seq).unwrap_or(checkpoint);

            let mut groups: HashMap<AccountId, Vec<EventEnvelope<AccountEvent, Account>>> =
                HashMap::new();
            for event in events {
                groups
                    .entry(AccountId::new(*event.envelope.id.as_ref()))
                    .or_default()
                    .push(event.envelope);
            }

            transaction.commit().await?;
            for (account_id, mut envelopes) in groups {
                let created_auth_for_permission = envelopes.iter().find_map(|event| {
                    if let AccountEvent::Created {
                        auth_account_id,
                        kind: AccountKind::Personal,
                        ..
                    } = &event.event
                    {
                        Some(auth_account_id.clone())
                    } else {
                        None
                    }
                });
                let mut create_permission = false;
                // One Account per bounded DB transaction; Keto is strictly after commit.
                // A Profile gate requested out of order (the Account's Profile set grew after the
                // set was settled) fails with `Concurrency`: roll back and redo the whole
                // transaction so the complete set is locked ascending from the start.
                let mut attempt = 0u8;
                loop {
                    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    let mut transaction = self.database_connection().get_transaction().await?;
                    self.home_link_store()
                        .lock_accounts(transaction.connection(), std::slice::from_ref(&account_id))
                        .await?;
                    // Shared lock order: Account gate -> Profile gates ascending -> relationship
                    // gates ascending -> links. The deactivation replay below and the
                    // reactivation restore further down use different Profile sets, so settle
                    // the union (incl. every Profile stream a restore may touch) *now*, before
                    // either step takes a Profile lock.
                    let may_deactivate = envelopes
                        .iter()
                        .any(|event| matches!(event.event, AccountEvent::Deactivated));
                    let may_restore = {
                        let executor = transaction.connection();
                        self.account_read_model()
                            .find_by_id_including_deleted(executor, &account_id)
                            .await?
                            .is_some_and(|account| account.deleted_at().is_some())
                    };
                    if may_deactivate || may_restore {
                        let history: Vec<ProfileId> = if may_restore {
                            let executor = transaction.connection();
                            self.profile_event_log()
                                .find_streams_touching_account(executor, &account_id)
                                .await?
                                .iter()
                                .map(|event| ProfileId::new(*event.envelope.id.as_ref()))
                                .collect()
                        } else {
                            Vec::new()
                        };
                        self.home_link_store()
                            .lock_account_profile_set(
                                transaction.connection(),
                                &account_id,
                                &history,
                            )
                            .await?;
                    }
                    for event in &envelopes {
                        if matches!(event.event, AccountEvent::Deactivated) {
                            self.home_link_store()
                                .deactivate(transaction.connection(), &account_id, &event.version)
                                .await?;
                        }
                    }
                    envelopes.sort_by_key(|event| *event.version.as_ref());
                    let created_auth_account_id = envelopes.iter().find_map(|event| {
                        if let AccountEvent::Created {
                            auth_account_id, ..
                        } = &event.event
                        {
                            Some(auth_account_id.clone())
                        } else {
                            None
                        }
                    });

                    // Including deactivated rows: a reactivation of an Account whose earlier events are
                    // outside the tailing window must fold onto the stored (deactivated) row. Without
                    // the row the window holds only the `Reactivated` event, the fold is "incomplete"
                    // and the Account would never come back.
                    let existing = {
                        let executor = transaction.connection();
                        self.account_read_model()
                            .find_by_id_including_deleted(executor, &account_id)
                            .await?
                    };
                    // The write-side stream is authoritative under the gate. A window may end
                    // on a stale deactivation after reactivation/new registration committed.
                    let entity = self
                        .home_link_store()
                        .account_snapshot(transaction.connection(), &account_id)
                        .await?;
                    if entity.is_none() {
                        transaction.commit().await?;
                        return Ok(());
                    }
                    create_permission = entity.as_ref().is_some_and(|a| {
                        a.deleted_at().is_none() && a.kind() == &AccountKind::Personal
                    });

                    let account_id_for_write = account_id.clone();
                    let savepoint = transaction.savepoint().await?;
                    let write_result: error_stack::Result<(), KernelError> = {
                        let executor = transaction.connection();
                        async {
                            let account_id = &account_id_for_write;
                            match (&entity, &existing) {
                                (Some(account), _) if account.deleted_at().is_some() => {
                                    self.account_projection_writer()
                                        .upsert(executor, account)
                                        .await?;
                                    self.home_link_store()
                                        .delete_owned_profiles(executor, account_id)
                                        .await?;
                                    let metadata_list = self
                                        .metadata_read_model()
                                        .find_by_account_id(executor, account_id)
                                        .await?;
                                    for metadata in &metadata_list {
                                        self.metadata_read_model()
                                            .delete(executor, metadata.id())
                                            .await?;
                                    }
                                    let target_id = FollowTargetId::from(account_id.clone());
                                    let followings = self
                                        .follow_repository()
                                        .find_followings(executor, &target_id)
                                        .await?;
                                    for follow in &followings {
                                        self.follow_repository()
                                            .delete(executor, follow.id())
                                            .await?;
                                    }
                                    let followers = self
                                        .follow_repository()
                                        .find_followers(executor, &target_id)
                                        .await?;
                                    for follow in &followers {
                                        self.follow_repository()
                                            .delete(executor, follow.id())
                                            .await?;
                                    }
                                    // auth_emumet_accounts linkage survives
                                    // deactivation: owners need it to reactivate.
                                }
                                (Some(account), prior) => {
                                    self.account_projection_writer()
                                        .upsert(executor, account)
                                        .await?;
                                    // Reactivation: deactivation deleted the Account's Profile rows, and
                                    // the Profile projector only looks at a window of recent events, so
                                    // a Profile whose events are older than that window would never come
                                    // back. Re-materialize the Profiles the Account owns from their full
                                    // history (Profile id, nanoid, username and key stay the same).
                                    if prior.as_ref().is_some_and(|p| p.deleted_at().is_some()) {
                                        self.restore_owned_profiles(executor, account_id).await?;
                                    }
                                    if let Some(auth_id) = created_auth_account_id {
                                        // Historical events can outlive the AuthAccount. Do not
                                        // resurrect a linkage/permission or stall all lifecycle
                                        // replay on that expected absence. Other DB errors propagate.
                                        if self
                                            .auth_account_repository()
                                            .find_by_id(executor, &auth_id)
                                            .await?
                                            .is_some()
                                        {
                                            self.account_read_model()
                                                .link_auth_account(executor, account_id, &auth_id)
                                                .await?;
                                        } else {
                                            create_permission = false;
                                            tracing::warn!(account_id = %account_id.as_ref(), "Account projection: historical AuthAccount absent; linkage skipped");
                                        }
                                    }
                                }
                                (None, _) => {
                                    tracing::warn!(
                                        "Account projector: entity is None for account {:?}",
                                        account_id
                                    );
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
                            return Err(error);
                        }
                    }
                    transaction.commit().await
                })
                .await
                .map_err(|_| {
                    error_stack::Report::new(KernelError::Internal)
                        .attach_printable("Account projection lifecycle transaction exceeded 5s")
                })?;
                    match outcome {
                        Err(error)
                            if matches!(error.current_context(), KernelError::Concurrency)
                                && attempt < 2 =>
                        {
                            attempt += 1;
                            tracing::warn!(account_id = %account_id.as_ref(), attempt, "Account projection: Profile set changed concurrently; retrying in a new transaction");
                        }
                        other => {
                            other?;
                            break;
                        }
                    }
                }
                if let Some(auth_id) = created_auth_for_permission.filter(|_| create_permission) {
                    self.permission_writer()
                        .create_relation(
                            &RelationTarget::Account {
                                account_id: account_id.clone(),
                                relation: AccountRelation::Owner,
                            },
                            &auth_id,
                        )
                        .await?;
                }
            }

            let mut transaction = self.database_connection().get_transaction().await?;
            let executor = transaction.connection();
            self.projection_checkpoint_store()
                .set(executor, ACCOUNT_PROJECTOR_NAME, max_seq)
                .await?;
            transaction.commit().await?;
            Ok(max_seq)
        }
    }

    /// Re-materializes, from the complete event history, every Profile currently owned by
    /// `account_id` (used when the Account is reactivated). Streams that were transferred away from
    /// the Account are skipped. The write is the version-gated upsert, so it cannot overwrite a row
    /// that is already newer.
    fn restore_owned_profiles<'a>(
        &'a self,
        executor: &'a mut <<Self as DependOnDatabaseConnection>::DatabaseConnection as kernel::interfaces::database::DatabaseConnection>::Connection,
        account_id: &'a AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            let events = self
                .profile_event_log()
                .find_streams_touching_account(executor, account_id)
                .await?;
            let mut streams: std::collections::BTreeMap<
                i64,
                Vec<EventEnvelope<kernel::prelude::entity::ProfileEvent, Profile>>,
            > = std::collections::BTreeMap::new();
            for event in events {
                streams
                    .entry(*event.envelope.id.as_ref())
                    .or_default()
                    .push(event.envelope);
            }
            self.home_link_store()
                .lock_profiles(
                    executor,
                    &streams
                        .keys()
                        .map(|id| ProfileId::new(*id))
                        .collect::<Vec<_>>(),
                )
                .await?;
            for (profile_id, mut envelopes) in streams {
                envelopes.sort_by_key(|event| *event.version.as_ref());
                let mut entity: Option<Profile> = None;
                let mut complete = true;
                for envelope in envelopes {
                    if let Err(error) = Profile::apply(&mut entity, envelope) {
                        tracing::warn!(
                            ?error,
                            profile_id,
                            "account reactivation: Profile stream could not be folded; skipped"
                        );
                        complete = false;
                        break;
                    }
                }
                let Some(profile) = entity.filter(|_| complete) else {
                    continue;
                };
                if profile.account_id() != account_id {
                    continue;
                }
                self.profile_projection_writer()
                    .upsert(executor, &profile)
                    .await?;
                tracing::info!(
                    profile_id = %ProfileId::new(profile_id).as_ref(),
                    "account reactivation: Profile re-materialized from its full history"
                );
            }
            Ok(())
        }
    }
}

impl<T> ProjectAccountBatch for T where
    T: DependOnDatabaseConnection<DatabaseConnection: TransactionalDatabaseConnection>
        + DependOnHomeLinkStore
        + DependOnAuthAccountRepository
        + DependOnAccountEventLog
        + DependOnProjectionCheckpointStore
        + DependOnAccountProjectionWriter
        + DependOnAccountReadModel
        + DependOnProfileReadModel
        + DependOnMetadataReadModel
        + DependOnFollowRepository
        + DependOnPermissionWriter
        + DependOnProfileEventLog
        + DependOnProfileProjectionWriter
{
}
