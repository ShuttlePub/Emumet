use kernel::interfaces::database::{
    DatabaseConnection, DependOnDatabaseConnection, Transaction, TransactionalDatabaseConnection,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{AccountEventStore, DependOnAccountEventStore};
use kernel::interfaces::permission::{
    AccountRelation, DependOnPermissionWriter, PermissionWriter, RelationTarget,
};
use kernel::interfaces::projection::{
    AccountEventLog, AccountProjectionWriter, DependOnAccountEventLog,
    DependOnAccountProjectionWriter, DependOnProjectionCheckpointStore,
    ProjectionCheckpointStore,
};
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnMetadataReadModel,
    DependOnProfileReadModel, MetadataReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{DependOnFollowRepository, FollowRepository};
use kernel::prelude::entity::{
    Account, AccountEvent, AccountId, EventEnvelope, EventId, FollowTargetId,
};
use kernel::KernelError;
use std::collections::HashMap;
use std::future::Future;

/// Orchestration that applies account events to the projection (ADR 0006
/// decision 6: the mediator lives in `application::projection`).
///
/// This is a pure move of the `AccountApplier` Redis-consumer closure from the
/// server crate: fetch the existing projection (unfiltered), rehydrate the
/// pending events, then branch the projection write — create + link + Keto
/// Owner provisioning / deactivate cascade / ban / suspend / update. No
/// behavior change in this commit (Redis driving, SQL and branching are
/// preserved as-is).
pub trait ProjectAccount:
    DependOnAccountReadModel
    + DependOnAccountEventStore
    + DependOnProfileReadModel
    + DependOnMetadataReadModel
    + DependOnFollowRepository
    + DependOnPermissionWriter
    + DependOnDatabaseConnection
{
    fn project_account(
        &self,
        account_id: AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut tx = self.database_connection().connection().await?;
            let event_id = EventId::from(account_id.clone());

            // 既存 Projection 取得 (unfiltered: suspended/banned も含める)
            let existing = self
                .account_read_model()
                .find_by_id_unfiltered(&mut tx, &account_id)
                .await?;
            let since_version = existing.as_ref().map(|a| a.version().clone());

            // 新規イベント取得
            let events = self
                .account_event_store()
                .find_by_id(&mut tx, &event_id, since_version.as_ref())
                .await?;
            if events.is_empty() {
                return Ok(());
            }

            // Created イベントから auth_account_id 抽出
            let mut auth_account_id_for_link = None;
            for event in &events {
                if let AccountEvent::Created {
                    auth_account_id, ..
                } = &event.event
                {
                    auth_account_id_for_link = Some(auth_account_id.clone());
                }
            }

            // イベント適用
            let mut entity = existing;
            for event in events {
                Account::apply(&mut entity, event)?;
            }

            // Projection 更新
            match (&entity, &since_version) {
                (Some(account), None) => {
                    self.account_read_model().create(&mut tx, account).await?;
                    if let Some(auth_id) = auth_account_id_for_link {
                        self.account_read_model()
                            .link_auth_account(&mut tx, &account_id, &auth_id)
                            .await?;

                        // Ensure Owner relation exists in Keto (idempotent upsert).
                        // This acts as a recovery mechanism if the permission_writer
                        // call in CreateAccountUseCase failed.
                        if let Err(e) = self
                            .permission_writer()
                            .create_relation(
                                &RelationTarget::Account {
                                    account_id: account_id.clone(),
                                    relation: AccountRelation::Owner,
                                },
                                &auth_id,
                            )
                            .await
                        {
                            tracing::warn!(
                                "Account projector: failed to create Owner relation for account {:?}: {:?}",
                                account_id,
                                e
                            );
                        }
                    }
                }
                (Some(account), Some(_)) => {
                    if account.deleted_at().is_some() {
                        // Account deactivated: cascade delete related data
                        self.account_read_model()
                            .deactivate(&mut tx, &account_id)
                            .await?;

                        // Delete profile
                        if let Some(profile) = self
                            .profile_read_model()
                            .find_by_account_id(&mut tx, &account_id)
                            .await?
                        {
                            self.profile_read_model()
                                .delete(&mut tx, profile.id())
                                .await?;
                        }

                        // Delete all metadata
                        let metadata_list = self
                            .metadata_read_model()
                            .find_by_account_id(&mut tx, &account_id)
                            .await?;
                        for metadata in &metadata_list {
                            self.metadata_read_model()
                                .delete(&mut tx, metadata.id())
                                .await?;
                        }

                        // Delete all follow relationships (as follower and followee)
                        let target_id = FollowTargetId::from(account_id.clone());
                        let followings = self
                            .follow_repository()
                            .find_followings(&mut tx, &target_id)
                            .await?;
                        for follow in &followings {
                            self.follow_repository()
                                .delete(&mut tx, follow.id())
                                .await?;
                        }
                        let followers = self
                            .follow_repository()
                            .find_followers(&mut tx, &target_id)
                            .await?;
                        for follow in &followers {
                            self.follow_repository()
                                .delete(&mut tx, follow.id())
                                .await?;
                        }

                        // Unlink all auth accounts
                        self.account_read_model()
                            .unlink_all_auth_accounts(&mut tx, &account_id)
                            .await?;
                    } else if account.status().is_banned() {
                        self.account_read_model()
                            .ban(
                                &mut tx,
                                &account_id,
                                match account.status() {
                                    kernel::prelude::entity::AccountStatus::Banned {
                                        reason,
                                        ..
                                    } => reason,
                                    _ => unreachable!(),
                                },
                            )
                            .await?;
                    } else if account.status().is_suspended() {
                        if let kernel::prelude::entity::AccountStatus::Suspended {
                            reason,
                            expires_at,
                            ..
                        } = account.status()
                        {
                            self.account_read_model()
                                .suspend(&mut tx, &account_id, reason, *expires_at)
                                .await?;
                        }
                    } else {
                        // Active or other: update() writes all columns including moderation fields
                        self.account_read_model()
                            .update(&mut tx, account)
                            .await?;
                    }
                }
                (None, Some(_)) => {
                    tracing::warn!(
                        "Account projector: entity became None with existing projection for id {:?} — this should not happen after Deactivated migration",
                        account_id
                    );
                }
                (None, None) => {
                    tracing::warn!(
                        "Account projector: entity is None with no prior projection for id {:?}",
                        account_id
                    );
                }
            }
            Ok(())
        }
    }
}

impl<T> ProjectAccount for T
where
    T: DependOnAccountReadModel
        + DependOnAccountEventStore
        + DependOnProfileReadModel
        + DependOnMetadataReadModel
        + DependOnFollowRepository
        + DependOnPermissionWriter
        + DependOnDatabaseConnection,
{
}

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
/// advances the checkpoint — all in one transaction.
pub trait ProjectAccountBatch:
    DependOnDatabaseConnection<DatabaseConnection: TransactionalDatabaseConnection>
    + DependOnAccountEventLog
    + DependOnProjectionCheckpointStore
    + DependOnAccountProjectionWriter
    + DependOnAccountReadModel
    + DependOnProfileReadModel
    + DependOnMetadataReadModel
    + DependOnFollowRepository
    + DependOnPermissionWriter
{
    /// Run one tailing poll. Returns the checkpoint after the poll.
    fn project_batch(&self) -> impl Future<Output = error_stack::Result<i64, KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_transaction().await?;
            let executor = transaction.connection();

            let checkpoint = self
                .projection_checkpoint_store()
                .get(executor, ACCOUNT_PROJECTOR_NAME)
                .await?
                .unwrap_or(0);
            let events = self
                .account_event_log()
                .find_by_seq_window(
                    executor,
                    checkpoint - ACCOUNT_PROJECTOR_WINDOW,
                    ACCOUNT_PROJECTOR_BATCH_LIMIT,
                )
                .await?;
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

            for (account_id, mut envelopes) in groups {
                envelopes.sort_by_key(|event| *event.version.as_ref());
                let created_auth_account_id = envelopes.iter().find_map(|event| {
                    if let AccountEvent::Created { auth_account_id, .. } = &event.event {
                        Some(auth_account_id.clone())
                    } else {
                        None
                    }
                });

                let existing = self
                    .account_read_model()
                    .find_by_id_unfiltered(executor, &account_id)
                    .await?;
                let pending: Vec<_> = envelopes
                    .into_iter()
                    .filter(|event| match &existing {
                        Some(account) => *event.version.as_ref() > *account.version().as_ref(),
                        None => true,
                    })
                    .collect();
                if pending.is_empty() && created_auth_account_id.is_none() {
                    continue;
                }

                let mut entity = existing.clone();
                let mut fold_failed = false;
                for event in &pending {
                    if let Err(error) = Account::apply(&mut entity, event.clone()) {
                        tracing::warn!(
                            ?error,
                            account_id = %account_id.as_ref(),
                            "projection fold skipped (incomplete stream); window re-read will retry"
                        );
                        fold_failed = true;
                        break;
                    }
                }
                if fold_failed {
                    continue;
                }

                match (&entity, &existing) {
                    (Some(account), _) if account.deleted_at().is_some() => {
                        // Deactivated: write the deleted state, then cascade.
                        self.account_projection_writer()
                            .upsert(executor, account)
                            .await?;
                        if let Some(profile) = self
                            .profile_read_model()
                            .find_by_account_id(executor, &account_id)
                            .await?
                        {
                            self.profile_read_model()
                                .delete(executor, profile.id())
                                .await?;
                        }
                        let metadata_list = self
                            .metadata_read_model()
                            .find_by_account_id(executor, &account_id)
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
                        self.account_read_model()
                            .unlink_all_auth_accounts(executor, &account_id)
                            .await?;
                    }
                    (Some(account), _) => {
                        self.account_projection_writer()
                            .upsert(executor, account)
                            .await?;
                        if let Some(auth_id) = created_auth_account_id {
                            self.account_read_model()
                                .link_auth_account(executor, &account_id, &auth_id)
                                .await?;
                            if let Err(e) = self
                                .permission_writer()
                                .create_relation(
                                    &RelationTarget::Account {
                                        account_id: account_id.clone(),
                                        relation: AccountRelation::Owner,
                                    },
                                    &auth_id,
                                )
                                .await
                            {
                                tracing::warn!(
                                    "Account projector: failed to create Owner relation for account {:?}: {:?}",
                                    account_id,
                                    e
                                );
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
            }

            self.projection_checkpoint_store()
                .set(executor, ACCOUNT_PROJECTOR_NAME, max_seq)
                .await?;
            transaction.commit().await?;
            Ok(max_seq)
        }
    }
}

impl<T> ProjectAccountBatch for T
where
    T: DependOnDatabaseConnection<DatabaseConnection: TransactionalDatabaseConnection>
        + DependOnAccountEventLog
        + DependOnProjectionCheckpointStore
        + DependOnAccountProjectionWriter
        + DependOnAccountReadModel
        + DependOnProfileReadModel
        + DependOnMetadataReadModel
        + DependOnFollowRepository
        + DependOnPermissionWriter,
{
}
