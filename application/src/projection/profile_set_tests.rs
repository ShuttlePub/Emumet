use super::*;
use kernel::interfaces::event_store::{DependOnProfileEventStore, ProfileEventStore};
use kernel::interfaces::home_link::{DependOnHomeLinkStore, HomeLinkStore};
use kernel::interfaces::home_link::{HomeLink, LinkOwner};
use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
use kernel::interfaces::repository::{DependOnProfileRelationshipLock, ProfileRelationshipLock};
use kernel::prelude::entity::{Profile, ProfileDisplayName, ProfileId};
use std::time::Duration;

async fn fixture(db: &PostgresDatabase) -> (Account, Profile) {
    let aid = AccountId::default();
    let auth = AuthAccountId::default();
    seed_auth_account(db, &auth).await;
    let a = fold(persist_account_events(db, &aid, &auth, false).await);
    let mut c = db.connection().await.unwrap();
    db.account_read_model().create(&mut c, &a).await.unwrap();
    let e = db
        .profile_event_store()
        .persist_and_transform(
            &mut c,
            Profile::create(
                ProfileId::new(kernel::generate_id()),
                aid,
                None,
                None,
                None,
                None,
                Nanoid::default(),
            ),
        )
        .await
        .unwrap();
    let mut p = None;
    Profile::apply(&mut p, e).unwrap();
    let p = p.unwrap();
    db.profile_read_model()
        .create_initial_personal_account_profile(&mut c, &p)
        .await
        .unwrap();
    (a, p)
}

async fn wait_blocked_by(db: &PostgresDatabase, blocker: i32) -> i32 {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let mut c = db.connection().await.unwrap();
            let pid: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)) LIMIT 1"
            ).bind(blocker).fetch_optional(&mut *c).await.unwrap();
            if let Some(pid) = pid { return pid; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("operation did not reach the expected lock stop point")
}

async fn absent(db: &PostgresDatabase, p: &Profile) {
    let mut c = db.connection().await.unwrap();
    assert!(
        db.profile_read_model()
            .find_by_id_unfiltered(&mut c, p.id())
            .await
            .unwrap()
            .is_none(),
        "deactivated owner's Profile must not be resurrected"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn same_owner_batch_update_cannot_resurrect_deleted_profile() {
    let _guard = PROJECTOR_TEST_LOCK.lock().await;
    kernel::ensure_generator_initialized();
    let db = PostgresDatabase::new().await.unwrap();
    clear_checkpoint(&db).await;
    let (a, p) = fixture(&db).await;
    let mut c = db.connection().await.unwrap();
    db.profile_event_store()
        .persist(
            &mut c,
            &Profile::update(
                p.id().clone(),
                kernel::prelude::entity::FieldAction::Set(ProfileDisplayName::new("pending")),
                kernel::prelude::entity::FieldAction::Unchanged,
                kernel::prelude::entity::FieldAction::Unchanged,
                kernel::prelude::entity::FieldAction::Unchanged,
            ),
        )
        .await
        .unwrap();
    db.account_event_store()
        .persist(
            &mut c,
            &Account::deactivate(a.id().clone(), a.version().clone()),
        )
        .await
        .unwrap();
    drop(c);
    // Account projection takes the Profile row, then stops at the relationship gate.
    let mut holder = db.get_transaction().await.unwrap();
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **holder.connection())
        .await
        .unwrap();
    db.profile_relationship_lock()
        .lock_profiles(holder.connection(), std::slice::from_ref(p.id()))
        .await
        .unwrap();
    let account_db = db.clone();
    let account =
        tokio::spawn(async move { ProjectorTest { db: account_db }.project_batch().await });
    let account_pid = wait_blocked_by(&db, holder_pid).await;
    let profile_db = db.clone();
    let profile = tokio::spawn(async move {
        ProjectorTest { db: profile_db }
            .project_profile_batch()
            .await
    });
    // Proves it planned against the live row and tried to write before Account deletion committed.
    wait_blocked_by(&db, account_pid).await;
    holder.commit().await.unwrap();
    account.await.unwrap().unwrap();
    profile.await.unwrap().unwrap();
    absent(&db, &p).await;
    ProjectorTest { db: db.clone() }
        .project_profile_batch()
        .await
        .unwrap();
    absent(&db, &p).await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn transferred_profile_stays_absent_after_current_owner_deactivation() {
    let _guard = PROJECTOR_TEST_LOCK.lock().await;
    kernel::ensure_generator_initialized();
    let db = PostgresDatabase::new().await.unwrap();
    clear_checkpoint(&db).await;
    let (a, p) = fixture(&db).await;
    let bid = AccountId::default();
    let auth = AuthAccountId::default();
    seed_auth_account(&db, &auth).await;
    let mut org = None;
    let mut oc = db.connection().await.unwrap();
    let created = db
        .account_event_store()
        .persist_and_transform(
            &mut oc,
            Account::create_organization(
                bid.clone(),
                AccountName::new(kernel::test_utils::unique_account_name()),
                AccountIsBot::new(false),
                Nanoid::default(),
                auth,
            ),
        )
        .await
        .unwrap();
    Account::apply(&mut org, created).unwrap();
    let b = org.unwrap();
    drop(oc);
    let mut c = db.connection().await.unwrap();
    db.account_read_model().create(&mut c, &b).await.unwrap();

    let e = db
        .profile_event_store()
        .persist_and_transform(
            &mut c,
            Profile::transfer_account(p.id().clone(), a.id().clone(), bid.clone()),
        )
        .await
        .unwrap();
    let mut moved = Some(p.clone());
    Profile::apply(&mut moved, e).unwrap();
    let mut tx = db.get_transaction().await.unwrap();
    db.home_link_store()
        .lock_accounts(tx.connection(), &[a.id().clone(), bid.clone()])
        .await
        .unwrap();
    db.home_link_store()
        .lock_profiles(tx.connection(), std::slice::from_ref(p.id()))
        .await
        .unwrap();
    db.profile_read_model()
        .update(tx.connection(), &moved.unwrap())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    db.account_event_store()
        .persist(&mut c, &Account::deactivate(bid, b.version().clone()))
        .await
        .unwrap();
    drop(c);
    let projector = ProjectorTest { db: db.clone() };
    projector.project_batch().await.unwrap();
    for _ in 0..3 {
        projector.project_profile_batch().await.unwrap();
        absent(&db, &p).await;
        projector.project_batch().await.unwrap();
        absent(&db, &p).await;
    }
}

async fn org_profiles(db: &PostgresDatabase) -> (Account, Vec<Profile>) {
    let aid = AccountId::default();
    let auth = AuthAccountId::default();
    seed_auth_account(db, &auth).await;
    let mut c = db.connection().await.unwrap();
    let event = db
        .account_event_store()
        .persist_and_transform(
            &mut c,
            Account::create_organization(
                aid.clone(),
                AccountName::new(kernel::test_utils::unique_account_name()),
                AccountIsBot::new(false),
                Nanoid::default(),
                auth,
            ),
        )
        .await
        .unwrap();
    let mut a = None;
    Account::apply(&mut a, event).unwrap();
    let a = a.unwrap();
    db.account_read_model().create(&mut c, &a).await.unwrap();
    let mut profiles = vec![];
    let mut tx = db.get_transaction().await.unwrap();
    db.home_link_store()
        .lock_accounts(tx.connection(), std::slice::from_ref(&aid))
        .await
        .unwrap();
    for _ in 0..2 {
        let event = db
            .profile_event_store()
            .persist_and_transform(
                tx.connection(),
                Profile::create(
                    ProfileId::new(kernel::generate_id()),
                    aid.clone(),
                    None,
                    None,
                    None,
                    None,
                    Nanoid::default(),
                ),
            )
            .await
            .unwrap();
        let mut p = None;
        Profile::apply(&mut p, event).unwrap();
        let p = p.unwrap();
        db.profile_read_model()
            .create(tx.connection(), &p)
            .await
            .unwrap();
        profiles.push(p);
    }
    tx.commit().await.unwrap();
    (a, profiles)
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn batch_content_updates_lock_rows_in_profile_id_order() {
    let _guard = PROJECTOR_TEST_LOCK.lock().await;
    kernel::ensure_generator_initialized();
    let db = PostgresDatabase::new().await.unwrap();
    clear_checkpoint(&db).await;
    let (_a, profiles) = org_profiles(&db).await;
    let mut c = db.connection().await.unwrap();
    // Append in reverse seq order: ordering must be by Profile ID, not seq or HashMap order.
    for p in profiles.iter().rev() {
        db.profile_event_store()
            .persist(
                &mut c,
                &Profile::update(
                    p.id().clone(),
                    kernel::prelude::entity::FieldAction::Set(ProfileDisplayName::new("new")),
                    kernel::prelude::entity::FieldAction::Unchanged,
                    kernel::prelude::entity::FieldAction::Unchanged,
                    kernel::prelude::entity::FieldAction::Unchanged,
                ),
            )
            .await
            .unwrap();
    }
    drop(c);
    let mut holder = db.get_transaction().await.unwrap();
    db.home_link_store()
        .lock_accounts(
            holder.connection(),
            std::slice::from_ref(profiles[0].account_id()),
        )
        .await
        .unwrap();
    db.home_link_store()
        .lock_profiles(holder.connection(), std::slice::from_ref(profiles[0].id()))
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **holder.connection())
        .await
        .unwrap();
    let other = db.clone();
    let task =
        tokio::spawn(async move { ProjectorTest { db: other }.project_profile_batch().await });
    wait_blocked_by(&db, pid).await;
    // If the batch locked high first, this NOWAIT would fail. The holder can finish its ascending
    // set while the batch is stopped on low, so no reverse-order cycle can form.
    sqlx::query("SELECT id FROM profiles WHERE id=$1 FOR NO KEY UPDATE NOWAIT")
        .bind(profiles[1].id().as_ref())
        .execute(&mut **holder.connection())
        .await
        .unwrap();
    db.home_link_store()
        .lock_profiles(holder.connection(), std::slice::from_ref(profiles[1].id()))
        .await
        .unwrap();
    holder.commit().await.unwrap();
    task.await.unwrap().unwrap();
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn zero_row_content_update_replans_missing_active_row_under_account_gate() {
    use kernel::interfaces::event_store::ProfileEventStore;
    let _guard = PROJECTOR_TEST_LOCK.lock().await;
    kernel::ensure_generator_initialized();
    let db = PostgresDatabase::new().await.unwrap();
    let (_a, p) = fixture(&db).await;
    let mut c = db.connection().await.unwrap();
    let event = db
        .profile_event_store()
        .persist_and_transform(
            &mut c,
            Profile::update(
                p.id().clone(),
                kernel::prelude::entity::FieldAction::Set(ProfileDisplayName::new("pending")),
                kernel::prelude::entity::FieldAction::Unchanged,
                kernel::prelude::entity::FieldAction::Unchanged,
                kernel::prelude::entity::FieldAction::Unchanged,
            ),
        )
        .await
        .unwrap();
    let events = db
        .profile_event_store()
        .find_by_id(&mut c, &EventId::from(p.id().clone()), None)
        .await
        .unwrap();
    let mut planned = Some(p.clone());
    Profile::apply(&mut planned, event).unwrap();
    sqlx::query("DELETE FROM profiles WHERE id=$1")
        .bind(p.id().as_ref())
        .execute(&mut *c)
        .await
        .unwrap();
    drop(c);
    let projector = ProjectorTest { db: db.clone() };
    let mut batch = db.get_transaction().await.unwrap();
    assert!(!projector
        .write_profile_group(&mut batch, p.id(), planned.unwrap(), &[])
        .await
        .unwrap());
    batch.commit().await.unwrap();
    let mut holder = db.get_transaction().await.unwrap();
    db.home_link_store()
        .lock_accounts(holder.connection(), std::slice::from_ref(p.account_id()))
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **holder.connection())
        .await
        .unwrap();
    let id = p.id().clone();
    let task =
        tokio::spawn(async move { projector.project_gated_profile_group(&id, &events).await });
    wait_blocked_by(&db, pid).await;
    absent(&db, &p).await;
    holder.commit().await.unwrap();
    task.await.unwrap().unwrap();
    assert!(db
        .profile_read_model()
        .find_by_id_unfiltered(&mut db.connection().await.unwrap(), p.id())
        .await
        .unwrap()
        .is_some());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn existing_row_is_not_moved_to_a_deactivated_owner() {
    let _guard = PROJECTOR_TEST_LOCK.lock().await;
    kernel::ensure_generator_initialized();
    let db = PostgresDatabase::new().await.unwrap();
    clear_checkpoint(&db).await;
    let (a, p) = fixture(&db).await;
    let (b, _) = org_profiles(&db).await;
    let mut c = db.connection().await.unwrap();
    db.profile_event_store()
        .persist(
            &mut c,
            &Profile::transfer_account(p.id().clone(), a.id().clone(), b.id().clone()),
        )
        .await
        .unwrap();
    db.account_event_store()
        .persist(
            &mut c,
            &Account::deactivate(b.id().clone(), b.version().clone()),
        )
        .await
        .unwrap();
    // Intentionally leave B's read model ACTIVE: the authoritative stream must veto the move.
    drop(c);
    ProjectorTest { db: db.clone() }
        .project_profile_batch()
        .await
        .unwrap();
    let stored = db
        .profile_read_model()
        .find_by_id_unfiltered(&mut db.connection().await.unwrap(), p.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.account_id(), a.id());
}

struct FailingProfileGate(std::sync::atomic::AtomicUsize, PostgresDatabase);
impl HomeLinkStore for FailingProfileGate {
    type Connection = driver::database::PostgresConnection;
    async fn lock_accounts(
        &self,
        connection: &mut Self::Connection,
        ids: &[AccountId],
    ) -> error_stack::Result<(), KernelError> {
        self.1
            .home_link_store()
            .lock_accounts(connection, ids)
            .await
    }
    async fn lock_profiles(
        &self,
        _connection: &mut Self::Connection,
        _ids: &[ProfileId],
    ) -> error_stack::Result<(), KernelError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(error_stack::Report::new(KernelError::Concurrency))
    }
    async fn lock_account_profile_set(
        &self,
        connection: &mut Self::Connection,
        account: &AccountId,
        extra: &[ProfileId],
    ) -> error_stack::Result<(), KernelError> {
        self.1
            .home_link_store()
            .lock_account_profile_set(connection, account, extra)
            .await
    }
    async fn owner_candidate(
        &self,
        connection: &mut Self::Connection,
        nanoid: &str,
    ) -> error_stack::Result<(ProfileId, AccountId), KernelError> {
        self.1
            .home_link_store()
            .owner_candidate(connection, nanoid)
            .await
    }
    async fn owner(
        &self,
        connection: &mut Self::Connection,
        profile: &ProfileId,
        candidate: &AccountId,
    ) -> error_stack::Result<LinkOwner, KernelError> {
        self.1
            .home_link_store()
            .owner(connection, profile, candidate)
            .await
    }
    async fn account_snapshot(
        &self,
        connection: &mut Self::Connection,
        account: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        self.1
            .home_link_store()
            .account_snapshot(connection, account)
            .await
    }
    async fn caller_owns(
        &self,
        connection: &mut Self::Connection,
        owner: &AccountId,
        auth: &AuthAccountId,
    ) -> error_stack::Result<bool, KernelError> {
        self.1
            .home_link_store()
            .caller_owns(connection, owner, auth)
            .await
    }
    async fn delete_owned_profiles(
        &self,
        connection: &mut Self::Connection,
        account: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        self.1
            .home_link_store()
            .delete_owned_profiles(connection, account)
            .await
    }
    async fn transfer_owner(
        &self,
        connection: &mut Self::Connection,
        profile: &ProfileId,
        to: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        self.1
            .home_link_store()
            .transfer_owner(connection, profile, to)
            .await
    }
    async fn list(
        &self,
        connection: &mut Self::Connection,
        profile: &ProfileId,
    ) -> error_stack::Result<Vec<HomeLink>, KernelError> {
        self.1.home_link_store().list(connection, profile).await
    }
    async fn register(
        &self,
        connection: &mut Self::Connection,
        owner: &LinkOwner,
        actor: &str,
        origin: &str,
    ) -> error_stack::Result<(HomeLink, bool), KernelError> {
        self.1
            .home_link_store()
            .register(connection, owner, actor, origin)
            .await
    }
    async fn unlink(
        &self,
        connection: &mut Self::Connection,
        profile: &ProfileId,
        id: &str,
    ) -> error_stack::Result<(), KernelError> {
        self.1
            .home_link_store()
            .unlink(connection, profile, id)
            .await
    }
    async fn deactivate(
        &self,
        connection: &mut Self::Connection,
        account: &AccountId,
        version: &EventVersion<Account>,
    ) -> error_stack::Result<(), KernelError> {
        self.1
            .home_link_store()
            .deactivate(connection, account, version)
            .await
    }
}
struct RetryProjector {
    db: PostgresDatabase,
    gate: FailingProfileGate,
}
impl kernel::interfaces::database::DependOnDatabaseConnection for RetryProjector {
    type DatabaseConnection = PostgresDatabase;
    fn database_connection(&self) -> &PostgresDatabase {
        &self.db
    }
}
impl DependOnHomeLinkStore for RetryProjector {
    type HomeLinkStore = FailingProfileGate;
    fn home_link_store(&self) -> &FailingProfileGate {
        &self.gate
    }
}
impl kernel::interfaces::read_model::DependOnAccountReadModel for RetryProjector {
    type AccountReadModel = <PostgresDatabase as kernel::interfaces::read_model::DependOnAccountReadModel>::AccountReadModel;
    fn account_read_model(&self) -> &Self::AccountReadModel {
        kernel::interfaces::read_model::DependOnAccountReadModel::account_read_model(&self.db)
    }
}
impl kernel::interfaces::read_model::DependOnProfileReadModel for RetryProjector {
    type ProfileReadModel = <PostgresDatabase as kernel::interfaces::read_model::DependOnProfileReadModel>::ProfileReadModel;
    fn profile_read_model(&self) -> &Self::ProfileReadModel {
        kernel::interfaces::read_model::DependOnProfileReadModel::profile_read_model(&self.db)
    }
}
impl kernel::interfaces::projection::DependOnProfileEventLog for RetryProjector {
    type ProfileEventLog = <PostgresDatabase as kernel::interfaces::projection::DependOnProfileEventLog>::ProfileEventLog;
    fn profile_event_log(&self) -> &Self::ProfileEventLog {
        kernel::interfaces::projection::DependOnProfileEventLog::profile_event_log(&self.db)
    }
}
impl kernel::interfaces::projection::DependOnProfileProjectionWriter for RetryProjector {
    type ProfileProjectionWriter = <PostgresDatabase as kernel::interfaces::projection::DependOnProfileProjectionWriter>::ProfileProjectionWriter;
    fn profile_projection_writer(&self) -> &Self::ProfileProjectionWriter {
        kernel::interfaces::projection::DependOnProfileProjectionWriter::profile_projection_writer(
            &self.db,
        )
    }
}
impl kernel::interfaces::projection::DependOnProjectionCheckpointStore for RetryProjector {
    type ProjectionCheckpointStore = <PostgresDatabase as kernel::interfaces::projection::DependOnProjectionCheckpointStore>::ProjectionCheckpointStore;
    fn projection_checkpoint_store(&self) -> &Self::ProjectionCheckpointStore {
        kernel::interfaces::projection::DependOnProjectionCheckpointStore::projection_checkpoint_store(&self.db)
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn exhausted_profile_gate_retries_do_not_advance_checkpoint() {
    use super::super::profile_projector::PROFILE_PROJECTOR_NAME;
    use kernel::interfaces::projection::{
        DependOnProjectionCheckpointStore, ProjectionCheckpointStore,
    };
    let _guard = PROJECTOR_TEST_LOCK.lock().await;
    kernel::ensure_generator_initialized();
    let iso = driver::database::isolated_db::IsolatedDatabase::create().await;
    let db = iso.database.clone();
    let (_a, p) = fixture(&db).await;
    let mut c = db.connection().await.unwrap();
    sqlx::query("DELETE FROM profiles WHERE id=$1")
        .bind(p.id().as_ref())
        .execute(&mut *c)
        .await
        .unwrap();
    let before = db
        .projection_checkpoint_store()
        .get(&mut c, PROFILE_PROJECTOR_NAME)
        .await
        .unwrap();
    drop(c);
    let projector = RetryProjector {
        db: db.clone(),
        gate: FailingProfileGate(std::sync::atomic::AtomicUsize::new(0), db.clone()),
    };
    let error = projector.project_profile_batch().await.unwrap_err();
    assert!(matches!(error.current_context(), KernelError::Concurrency));
    assert_eq!(
        projector.gate.0.load(std::sync::atomic::Ordering::SeqCst),
        3
    );
    let after = db
        .projection_checkpoint_store()
        .get(&mut db.connection().await.unwrap(), PROFILE_PROJECTOR_NAME)
        .await
        .unwrap();
    assert_eq!(before, after);
    absent(&db, &p).await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn zero_row_recheck_defers_changed_owner_but_skips_newer_same_owner() {
    use kernel::test_utils::ProfileBuilder;
    let _guard = PROJECTOR_TEST_LOCK.lock().await;
    kernel::ensure_generator_initialized();
    let db = PostgresDatabase::new().await.unwrap();
    let (a, profiles) = org_profiles(&db).await;
    let (b, _) = org_profiles(&db).await;
    let p = &profiles[0];
    let v = *p.version().as_ref();
    let moved = ProfileBuilder::new()
        .id(p.id().clone())
        .account_id(b.id().clone())
        .version(EventVersion::new(v + 10))
        .build();
    let mut tx = db.get_transaction().await.unwrap();
    db.home_link_store()
        .lock_accounts(tx.connection(), &[a.id().clone(), b.id().clone()])
        .await
        .unwrap();
    db.home_link_store()
        .lock_profiles(tx.connection(), std::slice::from_ref(p.id()))
        .await
        .unwrap();
    db.profile_read_model()
        .update(tx.connection(), &moved)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let projector = ProjectorTest { db: db.clone() };
    let mut tx = db.get_transaction().await.unwrap();
    assert!(!projector
        .write_profile_group(&mut tx, p.id(), p.clone(), &[])
        .await
        .unwrap());
    let older = ProfileBuilder::new()
        .id(p.id().clone())
        .account_id(b.id().clone())
        .version(EventVersion::new(v + 5))
        .build();
    assert!(projector
        .write_profile_group(&mut tx, p.id(), older, &[])
        .await
        .unwrap());
    tx.commit().await.unwrap();
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn account_reactivation_repairs_lagging_transfer_with_both_owner_gates() {
    let _guard = PROJECTOR_TEST_LOCK.lock().await;
    kernel::ensure_generator_initialized();
    let db = PostgresDatabase::new().await.unwrap();
    clear_checkpoint(&db).await;
    let (a, p) = fixture(&db).await;
    let (b, _) = org_profiles(&db).await;
    let mut c = db.connection().await.unwrap();
    db.profile_event_store()
        .persist(
            &mut c,
            &Profile::transfer_account(p.id().clone(), a.id().clone(), b.id().clone()),
        )
        .await
        .unwrap();
    let deleted = db
        .account_event_store()
        .persist_and_transform(
            &mut c,
            Account::deactivate(b.id().clone(), b.version().clone()),
        )
        .await
        .unwrap();
    // Simulate a lagging Profile projection while the Account read model already reflects withdrawal.
    db.account_read_model()
        .deactivate(&mut c, b.id())
        .await
        .unwrap();
    db.account_event_store()
        .persist(
            &mut c,
            &Account::reactivate(b.id().clone(), deleted.version),
        )
        .await
        .unwrap();
    drop(c);
    ProjectorTest { db: db.clone() }
        .project_batch()
        .await
        .unwrap();
    let stored = db
        .profile_read_model()
        .find_by_id_unfiltered(&mut db.connection().await.unwrap(), p.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.account_id(), b.id());
}
