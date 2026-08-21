use super::{ProjectAccountBatch, ProjectMetadataBatch, ProjectProfileBatch};
use driver::database::PostgresDatabase;
use kernel::impl_database_delegation;
use kernel::interfaces::database::{
    DatabaseConnection, Transaction, TransactionalDatabaseConnection,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{AccountEventStore, DependOnAccountEventStore};
use kernel::interfaces::permission::{DependOnPermissionWriter, PermissionWriter, RelationTarget};
use kernel::interfaces::projection::{AccountProjectionWriter, DependOnAccountProjectionWriter};
use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
use kernel::prelude::entity::{
    Account, AccountEvent, AccountId, AccountIsBot, AccountName, AuthAccountId, EventEnvelope,
    EventId, EventVersion, Nanoid,
};
use kernel::KernelError;
use serde_json::json;
use sqlx::PgConnection;
use tokio::sync::Mutex;

/// Projector tests share the `projection_checkpoints` row and the tailing
/// window, so they must not run concurrently.
static PROJECTOR_TEST_LOCK: Mutex<()> = Mutex::const_new(());

struct NoopPermissionWriter;

impl PermissionWriter for NoopPermissionWriter {
    async fn create_relation(
        &self,
        _target: &RelationTarget,
        _subject: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }

    async fn delete_relation(
        &self,
        _target: &RelationTarget,
        _subject: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }
}

struct ProjectorTest {
    db: PostgresDatabase,
}

impl_database_delegation!(ProjectorTest, db, PostgresDatabase);

impl DependOnPermissionWriter for ProjectorTest {
    type PermissionWriter = NoopPermissionWriter;

    fn permission_writer(&self) -> &Self::PermissionWriter {
        &NoopPermissionWriter
    }
}

fn fold(envelopes: Vec<EventEnvelope<AccountEvent, Account>>) -> Account {
    let mut entity = None;
    for envelope in envelopes {
        Account::apply(&mut entity, envelope).unwrap();
    }
    entity.expect("stream must fold into an account")
}

async fn clear_checkpoint(db: &PostgresDatabase) {
    let mut conn = db.connection().await.unwrap();
    let con: &mut PgConnection = &mut conn;
    sqlx::query("DELETE FROM projection_checkpoints")
        .execute(con)
        .await
        .unwrap();
}

async fn seed_auth_account(db: &PostgresDatabase, auth_account_id: &AuthAccountId) {
    let mut conn = db.connection().await.unwrap();
    let con: &mut PgConnection = &mut conn;
    let host_id = kernel::generate_id();
    sqlx::query("INSERT INTO auth_hosts (id, url) VALUES ($1, $2)")
        .bind(host_id)
        .bind(format!("https://auth-{host_id}.example.com"))
        .execute(&mut *con)
        .await
        .unwrap();
    sqlx::query("INSERT INTO auth_accounts (id, host_id, client_id) VALUES ($1, $2, $3)")
        .bind(auth_account_id.as_ref())
        .bind(host_id)
        .bind(format!("client-{host_id}"))
        .execute(&mut *con)
        .await
        .unwrap();
}

async fn max_seq_for(db: &PostgresDatabase, account_id: &AccountId) -> i64 {
    let mut conn = db.connection().await.unwrap();
    let con: &mut PgConnection = &mut conn;
    let row: Option<(i64,)> =
        sqlx::query_as::<_, (i64,)>("SELECT MAX(seq) FROM account_events WHERE id = $1")
            .bind(account_id.as_ref())
            .fetch_optional(con)
            .await
            .unwrap();
    row.map(|row| row.0).unwrap_or(0)
}

async fn persist_account_events(
    db: &PostgresDatabase,
    account_id: &AccountId,
    auth_account_id: &AuthAccountId,
    is_bot: bool,
) -> Vec<EventEnvelope<AccountEvent, Account>> {
    let mut conn = db.connection().await.unwrap();
    let create_command = Account::create(
        account_id.clone(),
        AccountName::new(kernel::test_utils::unique_account_name()),
        AccountIsBot::new(false),
        Nanoid::default(),
        auth_account_id.clone(),
    );
    let created_envelope = db
        .account_event_store()
        .persist_and_transform(&mut conn, create_command)
        .await
        .unwrap();
    let update_command = Account::update(
        account_id.clone(),
        AccountIsBot::new(is_bot),
        EventVersion::new(*created_envelope.version.as_ref()),
    );
    db.account_event_store()
        .persist(&mut conn, &update_command)
        .await
        .unwrap();
    drop(conn);
    let mut conn = db.connection().await.unwrap();
    db.account_event_store()
        .find_by_id(&mut conn, &EventId::from(account_id.clone()), None)
        .await
        .unwrap()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn project_batch_is_idempotent_and_advances_checkpoint() {
    let _projector_test_guard = PROJECTOR_TEST_LOCK.lock().await;
    kernel::ensure_generator_initialized();
    let projector = ProjectorTest {
        db: PostgresDatabase::new().await.unwrap(),
    };
    clear_checkpoint(&projector.db).await;
    let account_id = AccountId::default();
    let auth_account_id = AuthAccountId::default();
    seed_auth_account(&projector.db, &auth_account_id).await;
    let events = persist_account_events(&projector.db, &account_id, &auth_account_id, true).await;
    let account_max = max_seq_for(&projector.db, &account_id).await;
    let expected = fold(events);

    let checkpoint_1 = projector.project_batch().await.unwrap();
    let state_1 = projector
        .db
        .account_read_model()
        .find_by_id_unfiltered(&mut projector.db.connection().await.unwrap(), &account_id)
        .await
        .unwrap()
        .expect("first batch must materialize the account");

    let checkpoint_2 = projector.project_batch().await.unwrap();
    let state_2 = projector
        .db
        .account_read_model()
        .find_by_id_unfiltered(&mut projector.db.connection().await.unwrap(), &account_id)
        .await
        .unwrap()
        .expect("account must remain materialized");

    assert!(
        checkpoint_1 >= account_max,
        "checkpoint must cover the batch"
    );
    assert!(
        checkpoint_2 >= checkpoint_1,
        "checkpoint must never regress"
    );
    assert_eq!(state_1, expected, "first projection must match the fold");
    assert_eq!(state_2, expected, "re-apply must not change the read model");
    assert_eq!(state_2.version(), expected.version());
    assert_eq!(state_2.is_bot(), expected.is_bot());

    let mut conn = projector.db.connection().await.unwrap();
    let con: &mut PgConnection = &mut conn;
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM accounts WHERE id = $1")
        .bind(account_id.as_ref())
        .fetch_one(con)
        .await
        .unwrap();
    assert_eq!(count.0, 1, "re-apply must not insert a duplicate row");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn projected_deactivation_preserves_auth_account_linkage() {
    let _projector_test_guard = PROJECTOR_TEST_LOCK.lock().await;
    kernel::ensure_generator_initialized();
    let projector = ProjectorTest {
        db: PostgresDatabase::new().await.unwrap(),
    };
    clear_checkpoint(&projector.db).await;
    let account_id = AccountId::default();
    let auth_account_id = AuthAccountId::default();
    seed_auth_account(&projector.db, &auth_account_id).await;
    let events = persist_account_events(&projector.db, &account_id, &auth_account_id, false).await;
    let account_version = events.last().unwrap().version.clone();

    projector.project_batch().await.unwrap();
    let linked = projector
        .db
        .account_read_model()
        .find_auth_account_id_by_account_id(
            &mut projector.db.connection().await.unwrap(),
            &account_id,
        )
        .await
        .unwrap();
    assert_eq!(
        linked.as_ref(),
        Some(&auth_account_id),
        "projection must link the creator auth account"
    );

    let mut conn = projector.db.connection().await.unwrap();
    let deactivate = Account::deactivate(account_id.clone(), account_version);
    projector
        .db
        .account_event_store()
        .persist_and_transform(&mut conn, deactivate)
        .await
        .unwrap();
    drop(conn);

    projector.project_batch().await.unwrap();
    let linked = projector
        .db
        .account_read_model()
        .find_auth_account_id_by_account_id(
            &mut projector.db.connection().await.unwrap(),
            &account_id,
        )
        .await
        .unwrap();
    assert_eq!(
        linked.as_ref(),
        Some(&auth_account_id),
        "deactivation must not unlink auth accounts: owners need the linkage to reactivate"
    );

    let mut conn = projector.db.connection().await.unwrap();
    projector
        .db
        .account_read_model()
        .unlink_all_auth_accounts(&mut conn, &account_id)
        .await
        .unwrap();
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn older_version_upsert_is_a_noop() {
    let _projector_test_guard = PROJECTOR_TEST_LOCK.lock().await;
    kernel::ensure_generator_initialized();
    let projector = ProjectorTest {
        db: PostgresDatabase::new().await.unwrap(),
    };
    clear_checkpoint(&projector.db).await;
    let account_id = AccountId::default();
    let account_v2 = fold(vec![
        EventEnvelope::new(
            EventId::from(account_id.clone()),
            AccountEvent::Created {
                name: AccountName::new(kernel::test_utils::unique_account_name()),
                is_bot: AccountIsBot::new(false),
                nanoid: Nanoid::default(),
                auth_account_id: AuthAccountId::default(),
            },
            EventVersion::new(1),
        ),
        EventEnvelope::new(
            EventId::from(account_id.clone()),
            AccountEvent::Updated {
                is_bot: AccountIsBot::new(true),
            },
            EventVersion::new(2),
        ),
    ]);
    let account_v1 = fold(vec![EventEnvelope::new(
        EventId::from(account_id.clone()),
        AccountEvent::Created {
            name: AccountName::new(kernel::test_utils::unique_account_name()),
            is_bot: AccountIsBot::new(false),
            nanoid: Nanoid::default(),
            auth_account_id: AuthAccountId::default(),
        },
        EventVersion::new(1),
    )]);

    projector
        .db
        .account_projection_writer()
        .upsert(&mut projector.db.connection().await.unwrap(), &account_v2)
        .await
        .unwrap();
    projector
        .db
        .account_projection_writer()
        .upsert(&mut projector.db.connection().await.unwrap(), &account_v1)
        .await
        .unwrap();

    let stored = projector
        .db
        .account_read_model()
        .find_by_id_unfiltered(&mut projector.db.connection().await.unwrap(), &account_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.version(),
        &EventVersion::new(2),
        "older version must not overwrite a newer projection"
    );
    assert_eq!(stored.is_bot(), &AccountIsBot::new(true));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn commit_order_inversion_eventually_projects_both_events() {
    let _projector_test_guard = PROJECTOR_TEST_LOCK.lock().await;
    kernel::ensure_generator_initialized();
    let projector = ProjectorTest {
        db: PostgresDatabase::new().await.unwrap(),
    };
    clear_checkpoint(&projector.db).await;
    let account_id = AccountId::default();
    let auth_account_id = AuthAccountId::default();
    seed_auth_account(&projector.db, &auth_account_id).await;

    let mut tx_create = projector.db.get_transaction().await.unwrap();
    let account_id_value: i64 = *account_id.as_ref();
    let created = json!({
        "type": "Created",
        "name": kernel::test_utils::unique_account_name(),
        "is_bot": false,
        "nanoid": Nanoid::<Account>::default().as_ref(),
        "auth_account_id": auth_account_id.as_ref(),
    });
    let create_con: &mut PgConnection = tx_create.connection();
    sqlx::query(
        "INSERT INTO account_events (version, id, event_name, data) VALUES ($1, $2, $3, $4)",
    )
    .bind(1_i64)
    .bind(account_id_value)
    .bind("account_created")
    .bind(&created)
    .execute(&mut *create_con)
    .await
    .unwrap();

    let mut tx_update = projector.db.get_transaction().await.unwrap();
    let updated = json!({ "type": "Updated".to_string(), "is_bot": true });
    let update_con: &mut PgConnection = tx_update.connection();
    sqlx::query(
        "INSERT INTO account_events (version, id, event_name, data) VALUES ($1, $2, $3, $4)",
    )
    .bind(2_i64)
    .bind(account_id_value)
    .bind("account_updated")
    .bind(&updated)
    .execute(&mut *update_con)
    .await
    .unwrap();
    tx_update.commit().await.unwrap();

    let checkpoint_1 = projector.project_batch().await.unwrap();
    assert!(
        checkpoint_1 >= max_seq_for(&projector.db, &account_id).await,
        "checkpoint must advance past the committed event"
    );

    tx_create.commit().await.unwrap();
    let checkpoint_2 = projector.project_batch().await.unwrap();
    assert!(
        checkpoint_2 >= checkpoint_1,
        "checkpoint must never regress after the straggler commits"
    );

    let projected = projector
        .db
        .account_read_model()
        .find_by_id_unfiltered(&mut projector.db.connection().await.unwrap(), &account_id)
        .await
        .unwrap()
        .expect("window re-read must project the late-committed create");
    assert_eq!(
        projected.version(),
        &EventVersion::new(2),
        "both events must be projected"
    );
    assert_eq!(projected.is_bot(), &AccountIsBot::new(true));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn per_aggregate_fold_uses_version_order_not_seq_order() {
    let _projector_test_guard = PROJECTOR_TEST_LOCK.lock().await;
    kernel::ensure_generator_initialized();
    let projector = ProjectorTest {
        db: PostgresDatabase::new().await.unwrap(),
    };
    clear_checkpoint(&projector.db).await;
    let account_id = AccountId::default();
    let auth_account_id = AuthAccountId::default();
    seed_auth_account(&projector.db, &auth_account_id).await;
    let account_id_value: i64 = *account_id.as_ref();

    let mut conn = projector.db.connection().await.unwrap();
    let con: &mut PgConnection = &mut conn;
    // seq order (insert order) is the inverse of version order: the Updated
    // event (version 2) is inserted before the Created event (version 1).
    let updated = json!({ "type": "Updated".to_string(), "is_bot": true });
    sqlx::query(
        "INSERT INTO account_events (version, id, event_name, data) VALUES ($1, $2, $3, $4)",
    )
    .bind(2_i64)
    .bind(account_id_value)
    .bind("account_updated")
    .bind(&updated)
    .execute(&mut *con)
    .await
    .unwrap();
    let created = json!({
        "type": "Created",
        "name": kernel::test_utils::unique_account_name(),
        "is_bot": false,
        "nanoid": Nanoid::<Account>::default().as_ref(),
        "auth_account_id": auth_account_id.as_ref(),
    });
    sqlx::query(
        "INSERT INTO account_events (version, id, event_name, data) VALUES ($1, $2, $3, $4)",
    )
    .bind(1_i64)
    .bind(account_id_value)
    .bind("account_created")
    .bind(&created)
    .execute(&mut *con)
    .await
    .unwrap();
    drop(conn);

    projector.project_batch().await.unwrap();

    let projected = projector
        .db
        .account_read_model()
        .find_by_id_unfiltered(&mut projector.db.connection().await.unwrap(), &account_id)
        .await
        .unwrap()
        .expect("version-ordered fold must converge");
    assert_eq!(projected.version(), &EventVersion::new(2));
    assert_eq!(projected.is_bot(), &AccountIsBot::new(true));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn checkpoint_advances_as_events_are_appended() {
    let _projector_test_guard = PROJECTOR_TEST_LOCK.lock().await;
    kernel::ensure_generator_initialized();
    let projector = ProjectorTest {
        db: PostgresDatabase::new().await.unwrap(),
    };
    clear_checkpoint(&projector.db).await;
    let account_id = AccountId::default();
    let auth_account_id = AuthAccountId::default();
    seed_auth_account(&projector.db, &auth_account_id).await;

    let mut conn = projector.db.connection().await.unwrap();
    let create_command = Account::create(
        account_id.clone(),
        AccountName::new(kernel::test_utils::unique_account_name()),
        AccountIsBot::new(false),
        Nanoid::default(),
        auth_account_id.clone(),
    );
    let created_envelope = projector
        .db
        .account_event_store()
        .persist_and_transform(&mut conn, create_command)
        .await
        .unwrap();
    drop(conn);

    let checkpoint_1 = projector.project_batch().await.unwrap();
    let max_1 = max_seq_for(&projector.db, &account_id).await;
    assert!(checkpoint_1 >= max_1);

    let mut conn = projector.db.connection().await.unwrap();
    let update_command = Account::update(
        account_id.clone(),
        AccountIsBot::new(true),
        EventVersion::new(*created_envelope.version.as_ref()),
    );
    projector
        .db
        .account_event_store()
        .persist(&mut conn, &update_command)
        .await
        .unwrap();
    drop(conn);

    let checkpoint_2 = projector.project_batch().await.unwrap();
    let max_2 = max_seq_for(&projector.db, &account_id).await;
    assert!(checkpoint_2 >= max_2);
    assert!(
        checkpoint_2 > checkpoint_1,
        "checkpoint must advance when new events are appended"
    );
}

mod profile {
    use super::{clear_checkpoint, ProjectAccountBatch, ProjectProfileBatch};
    use driver::database::PostgresDatabase;
    use kernel::impl_database_delegation;
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::event_store::{
        AccountEventStore, DependOnAccountEventStore, DependOnProfileEventStore, ProfileEventStore,
    };

    use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
    use kernel::prelude::entity::{
        Account, AccountId, AccountIsBot, AccountName, AuthAccountId, Nanoid, Profile,
        ProfileDisplayName, ProfileId, ProfileSummary,
    };

    struct ProfileProjectorTest {
        db: PostgresDatabase,
    }

    impl_database_delegation!(ProfileProjectorTest, db, PostgresDatabase);

    async fn seed_profile(db: &PostgresDatabase) -> (ProfileId, Profile) {
        let mut conn = db.connection().await.unwrap();
        let profile_id = ProfileId::new(kernel::generate_id());
        let account_id = AccountId::new(kernel::generate_id());
        let account_name = format!("test-{}", account_id.as_ref());
        sqlx::query(
            //language=postgresql
            "INSERT INTO accounts (id, name, is_bot, version, nanoid, created_at) \
             VALUES ($1, $2, false, 1, $3, NOW())",
        )
        .bind(account_id.as_ref())
        .bind(&account_name)
        .bind(format!("nanoid-{}", account_id.as_ref()))
        .execute(&mut *conn)
        .await
        .unwrap();
        let command = Profile::create(
            profile_id.clone(),
            account_id,
            Some(ProfileDisplayName::new("old name".to_string())),
            Some(ProfileSummary::new("old summary".to_string())),
            None,
            None,
            kernel::prelude::entity::Nanoid::<Profile>::default(),
        );
        let created = db
            .profile_event_store()
            .persist_and_transform(&mut conn, command)
            .await
            .unwrap();
        let mut profile = None;
        kernel::interfaces::event::EventApplier::apply(&mut profile, created).unwrap();
        (profile_id, profile.unwrap())
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn profile_projector_materializes_and_is_idempotent() {
        let _guard = super::PROJECTOR_TEST_LOCK.lock().await;
        kernel::ensure_generator_initialized();
        let projector = ProfileProjectorTest {
            db: PostgresDatabase::new().await.unwrap(),
        };
        clear_checkpoint(&projector.db).await;
        let (profile_id, expected) = seed_profile(&projector.db).await;
        assert_eq!(expected.id(), &profile_id);

        let checkpoint_1 = projector.project_profile_batch().await.unwrap();
        let state_1 = projector
            .db
            .profile_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &profile_id)
            .await
            .unwrap()
            .expect("profile must be materialized");
        let checkpoint_2 = projector.project_profile_batch().await.unwrap();
        let state_2 = projector
            .db
            .profile_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &profile_id)
            .await
            .unwrap()
            .unwrap();

        assert!(checkpoint_2 >= checkpoint_1);
        assert_eq!(state_1.display_name(), expected.display_name());
        assert_eq!(state_2.display_name(), expected.display_name());
    }
    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn profile_projector_applies_out_of_window_update_onto_existing_projection() {
        let _guard = super::PROJECTOR_TEST_LOCK.lock().await;
        kernel::ensure_generator_initialized();
        let projector = ProfileProjectorTest {
            db: PostgresDatabase::new().await.unwrap(),
        };
        clear_checkpoint(&projector.db).await;
        let (profile_id, _existing) = seed_profile(&projector.db).await;

        projector.project_profile_batch().await.unwrap();
        let before = projector
            .db
            .profile_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &profile_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            before.display_name().as_ref().map(|v| v.as_ref().as_str()),
            Some("old name")
        );

        // Move the event sequence and checkpoint far past the Created event so the
        // next batch no longer sees it, then append an Updated event.
        let mut conn = projector.db.connection().await.unwrap();
        // The Created event's seq (and the current max seq) is the lower bound
        // that must sit outside the projector window.  Query it instead of
        // assuming a fresh DB so the test is robust to reruns.
        let created_seq: i64 = sqlx::query_scalar(
            //language=postgresql
            "SELECT MAX(seq) FROM profile_events WHERE id = $1",
        )
        .bind(profile_id.as_ref())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            //language=postgresql
            "SELECT setval(pg_get_serial_sequence('profile_events', 'seq'), $1)",
        )
        .bind(created_seq + 100_000)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            //language=postgresql
            "UPDATE projection_checkpoints SET last_seq = $1 WHERE projector_name = $2",
        )
        .bind(created_seq + 99_999)
        .bind("profile_projector")
        .execute(&mut *conn)
        .await
        .unwrap();
        let update = Profile::update(
            profile_id.clone(),
            kernel::prelude::entity::FieldAction::Set(ProfileDisplayName::new(
                "updated name".to_string(),
            )),
            kernel::prelude::entity::FieldAction::Unchanged,
            kernel::prelude::entity::FieldAction::Unchanged,
            kernel::prelude::entity::FieldAction::Unchanged,
        );
        projector
            .db
            .profile_event_store()
            .persist(&mut conn, &update)
            .await
            .unwrap();
        drop(conn);

        projector.project_profile_batch().await.unwrap();
        let after = projector
            .db
            .profile_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &profile_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            after.display_name().as_ref().map(|v| v.as_ref().as_str()),
            Some("updated name")
        );
    }
    /// Regression test: when the Account projector cascade-deletes a profile
    /// row, the Profile projector must not resurrect it on the next tick.
    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn profile_projector_skips_deleted_account_on_fresh_materialization() {
        let _guard = super::PROJECTOR_TEST_LOCK.lock().await;
        kernel::ensure_generator_initialized();
        let projector = super::ProjectorTest {
            db: PostgresDatabase::new().await.unwrap(),
        };
        super::clear_checkpoint(&projector.db).await;

        // 1. Seed the auth account and create an account via the event store.
        let auth_account_id = AuthAccountId::new(kernel::generate_id());
        super::seed_auth_account(&projector.db, &auth_account_id).await;
        let mut conn = projector.db.connection().await.unwrap();
        let account_id = AccountId::new(kernel::generate_id());
        let create_account = Account::create(
            account_id.clone(),
            AccountName::new(format!("testuser-{}", account_id.as_ref())),
            AccountIsBot::new(false),
            Nanoid::<Account>::default(),
            auth_account_id.clone(),
        );
        let envelope = projector
            .db
            .account_event_store()
            .persist_and_transform(&mut conn, create_account)
            .await
            .unwrap();
        let account_version = envelope.version.clone();
        drop(conn);

        // 2. Project the account so the read-model row exists.
        let ckpt_before = projector.project_batch().await.unwrap();
        assert!(ckpt_before > 0);

        // 3. Create a profile for the account via event store.
        let mut conn = projector.db.connection().await.unwrap();
        let profile_id = ProfileId::new(kernel::generate_id());
        let create_profile = Profile::create(
            profile_id.clone(),
            account_id.clone(),
            Some(ProfileDisplayName::new("test".to_string())),
            None,
            None,
            None,
            Nanoid::<Profile>::default(),
        );
        projector
            .db
            .profile_event_store()
            .persist(&mut conn, &create_profile)
            .await
            .unwrap();
        drop(conn);

        // 4. Project the profile — row should appear.
        projector.project_profile_batch().await.unwrap();
        let found = projector
            .db
            .profile_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &profile_id)
            .await
            .unwrap();
        assert!(found.is_some(), "profile row should exist before delete");

        // 5. Delete the account via event store.
        let mut conn = projector.db.connection().await.unwrap();
        let deactivate = Account::deactivate(account_id.clone(), account_version);
        projector
            .db
            .account_event_store()
            .persist_and_transform(&mut conn, deactivate)
            .await
            .unwrap();
        drop(conn);

        // 6. Project the account — cascade-deletes the profile row.
        projector.project_batch().await.unwrap();
        let gone = projector
            .db
            .profile_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &profile_id)
            .await
            .unwrap();
        assert!(
            gone.is_none(),
            "profile row should be gone after account cascade-delete"
        );

        // 7. Re-project the profile — the Created event is still in the
        //    window, but the account is deleted so the row must NOT come back.
        projector.project_profile_batch().await.unwrap();
        let resurrected = projector
            .db
            .profile_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &profile_id)
            .await
            .unwrap();
        assert!(
            resurrected.is_none(),
            "profile row must not be resurrected after account deletion"
        );
    }
}

mod metadata {
    use super::{clear_checkpoint, ProjectAccountBatch, ProjectMetadataBatch};
    use driver::database::PostgresDatabase;
    use kernel::impl_database_delegation;
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::event_store::{
        AccountEventStore, DependOnAccountEventStore, DependOnMetadataEventStore,
        MetadataEventStore,
    };

    use kernel::interfaces::read_model::{DependOnMetadataReadModel, MetadataReadModel};
    use kernel::prelude::entity::{
        Account, AccountId, AccountIsBot, AccountName, AuthAccountId, Metadata, MetadataContent,
        MetadataId, MetadataLabel, Nanoid,
    };

    struct MetadataProjectorTest {
        db: PostgresDatabase,
    }

    impl_database_delegation!(MetadataProjectorTest, db, PostgresDatabase);

    async fn seed_metadata(db: &PostgresDatabase) -> (MetadataId, Metadata) {
        let mut conn = db.connection().await.unwrap();
        let metadata_id = MetadataId::new(kernel::generate_id());
        let account_id = AccountId::new(kernel::generate_id());
        let account_name = format!("test-{}", account_id.as_ref());
        sqlx::query(
            //language=postgresql
            "INSERT INTO accounts (id, name, is_bot, version, nanoid, created_at) \
             VALUES ($1, $2, false, 1, $3, NOW())",
        )
        .bind(account_id.as_ref())
        .bind(&account_name)
        .bind(format!("nanoid-{}", account_id.as_ref()))
        .execute(&mut *conn)
        .await
        .unwrap();
        let command = Metadata::create(
            metadata_id.clone(),
            account_id,
            MetadataLabel::new("label".to_string()),
            MetadataContent::new("content".to_string()),
            kernel::prelude::entity::Nanoid::<Metadata>::default(),
        );
        let created = db
            .metadata_event_store()
            .persist_and_transform(&mut conn, command)
            .await
            .unwrap();
        let mut metadata = None;
        kernel::interfaces::event::EventApplier::apply(&mut metadata, created).unwrap();
        (metadata_id, metadata.unwrap())
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn metadata_projector_materializes_and_is_idempotent() {
        let _guard = super::PROJECTOR_TEST_LOCK.lock().await;
        kernel::ensure_generator_initialized();
        let projector = MetadataProjectorTest {
            db: PostgresDatabase::new().await.unwrap(),
        };
        clear_checkpoint(&projector.db).await;
        let (metadata_id, expected) = seed_metadata(&projector.db).await;

        let checkpoint_1 = projector.project_metadata_batch().await.unwrap();
        let state_1 = projector
            .db
            .metadata_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &metadata_id)
            .await
            .unwrap()
            .expect("metadata must be materialized");
        let checkpoint_2 = projector.project_metadata_batch().await.unwrap();
        let state_2 = projector
            .db
            .metadata_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &metadata_id)
            .await
            .unwrap()
            .unwrap();

        assert!(checkpoint_2 >= checkpoint_1);
        assert_eq!(state_1.label(), expected.label());
        assert_eq!(state_2.label(), expected.label());
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn metadata_projector_deletes_on_deleted_event() {
        let _guard = super::PROJECTOR_TEST_LOCK.lock().await;
        kernel::ensure_generator_initialized();
        let projector = MetadataProjectorTest {
            db: PostgresDatabase::new().await.unwrap(),
        };
        clear_checkpoint(&projector.db).await;
        let (metadata_id, expected) = seed_metadata(&projector.db).await;

        projector.project_metadata_batch().await.unwrap();
        let exists = projector
            .db
            .metadata_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &metadata_id)
            .await
            .unwrap()
            .is_some();
        assert!(exists);

        let mut conn = projector.db.connection().await.unwrap();
        let delete = Metadata::delete(metadata_id.clone(), expected.version().clone());
        projector
            .db
            .metadata_event_store()
            .persist(&mut conn, &delete)
            .await
            .unwrap();
        drop(conn);

        projector.project_metadata_batch().await.unwrap();
        let gone = projector
            .db
            .metadata_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &metadata_id)
            .await
            .unwrap();
        assert!(gone.is_none());
    }
    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn metadata_projector_applies_out_of_window_update_onto_existing_projection() {
        let _guard = super::PROJECTOR_TEST_LOCK.lock().await;
        kernel::ensure_generator_initialized();
        let projector = MetadataProjectorTest {
            db: PostgresDatabase::new().await.unwrap(),
        };
        clear_checkpoint(&projector.db).await;
        let (metadata_id, existing) = seed_metadata(&projector.db).await;

        projector.project_metadata_batch().await.unwrap();
        let before = projector
            .db
            .metadata_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &metadata_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.label().as_ref(), "label");

        let mut conn = projector.db.connection().await.unwrap();
        let created_seq: i64 = sqlx::query_scalar(
            //language=postgresql
            "SELECT MAX(seq) FROM metadata_events WHERE id = $1",
        )
        .bind(metadata_id.as_ref())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            //language=postgresql
            "SELECT setval(pg_get_serial_sequence('metadata_events', 'seq'), $1)",
        )
        .bind(created_seq + 100_000)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            //language=postgresql
            "UPDATE projection_checkpoints SET last_seq = $1 WHERE projector_name = $2",
        )
        .bind(created_seq + 99_999)
        .bind("metadata_projector")
        .execute(&mut *conn)
        .await
        .unwrap();
        let update = Metadata::update(
            metadata_id.clone(),
            MetadataLabel::new("Updated Site".to_string()),
            MetadataContent::new("https://example.com/updated".to_string()),
            existing.version().clone(),
        );
        projector
            .db
            .metadata_event_store()
            .persist(&mut conn, &update)
            .await
            .unwrap();
        drop(conn);

        projector.project_metadata_batch().await.unwrap();
        let after = projector
            .db
            .metadata_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &metadata_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.label().as_ref(), "Updated Site");
    }
    /// Regression test: when the Account projector cascade-deletes a metadata
    /// row, the Metadata projector must not resurrect it on the next tick.
    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn metadata_projector_skips_deleted_account_on_fresh_materialization() {
        let _guard = super::PROJECTOR_TEST_LOCK.lock().await;
        kernel::ensure_generator_initialized();
        let projector = MetadataProjectorTest {
            db: PostgresDatabase::new().await.unwrap(),
        };
        super::clear_checkpoint(&projector.db).await;

        // 1. Seed the auth account and create an account via the event store
        //    using the shared ProjectorTest which has AccountEventLog.
        let shared = super::ProjectorTest {
            db: PostgresDatabase::new().await.unwrap(),
        };
        let auth_account_id = AuthAccountId::new(kernel::generate_id());
        super::seed_auth_account(&shared.db, &auth_account_id).await;
        let mut conn = shared.db.connection().await.unwrap();
        let account_id = AccountId::new(kernel::generate_id());
        let create_account = Account::create(
            account_id.clone(),
            AccountName::new(format!("metauser-{}", account_id.as_ref())),
            AccountIsBot::new(false),
            Nanoid::<Account>::default(),
            auth_account_id.clone(),
        );
        let envelope = shared
            .db
            .account_event_store()
            .persist_and_transform(&mut conn, create_account)
            .await
            .unwrap();
        let account_version = envelope.version.clone();
        drop(conn);

        // 2. Project the account so the read-model row exists.
        shared.project_batch().await.unwrap();

        // 3. Create a metadata via event store.
        let mut conn = projector.db.connection().await.unwrap();
        let metadata_id = MetadataId::new(kernel::generate_id());
        let create_metadata = Metadata::create(
            metadata_id.clone(),
            account_id.clone(),
            MetadataLabel::new("Website".to_string()),
            MetadataContent::new("https://example.com".to_string()),
            Nanoid::<Metadata>::default(),
        );
        projector
            .db
            .metadata_event_store()
            .persist(&mut conn, &create_metadata)
            .await
            .unwrap();
        drop(conn);

        // 4. Project the metadata — row should appear.
        projector.project_metadata_batch().await.unwrap();
        let found = projector
            .db
            .metadata_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &metadata_id)
            .await
            .unwrap();
        assert!(found.is_some(), "metadata row should exist before delete");

        // 5. Delete the account via event store.
        let mut conn = shared.db.connection().await.unwrap();
        let deactivate = Account::deactivate(account_id.clone(), account_version);
        shared
            .db
            .account_event_store()
            .persist_and_transform(&mut conn, deactivate)
            .await
            .unwrap();
        drop(conn);

        // 6. Project the account — cascade-deletes the metadata row.
        shared.project_batch().await.unwrap();
        let gone = projector
            .db
            .metadata_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &metadata_id)
            .await
            .unwrap();
        assert!(
            gone.is_none(),
            "metadata row should be gone after account cascade-delete"
        );

        // 7. Re-project the metadata — Created event is in window, but
        //    account is deleted so row must NOT come back.
        projector.project_metadata_batch().await.unwrap();
        let resurrected = projector
            .db
            .metadata_read_model()
            .find_by_id(&mut projector.db.connection().await.unwrap(), &metadata_id)
            .await
            .unwrap();
        assert!(
            resurrected.is_none(),
            "metadata row must not be resurrected after account deletion"
        );
    }
}
