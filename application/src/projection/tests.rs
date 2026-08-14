use super::ProjectAccountBatch;
use driver::database::PostgresDatabase;
use kernel::impl_database_delegation;
use kernel::interfaces::database::{
    DatabaseConnection, Transaction, TransactionalDatabaseConnection,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{AccountEventStore, DependOnAccountEventStore};
use kernel::interfaces::permission::{DependOnPermissionWriter, PermissionWriter, RelationTarget};
use kernel::interfaces::projection::{
    AccountProjectionWriter, DependOnAccountEventLog, DependOnAccountProjectionWriter,
    DependOnProjectionCheckpointStore,
};
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

impl DependOnAccountEventLog for ProjectorTest {
    type AccountEventLog = <PostgresDatabase as DependOnAccountEventLog>::AccountEventLog;

    fn account_event_log(&self) -> &Self::AccountEventLog {
        DependOnAccountEventLog::account_event_log(&self.db)
    }
}

impl DependOnProjectionCheckpointStore for ProjectorTest {
    type ProjectionCheckpointStore =
        <PostgresDatabase as DependOnProjectionCheckpointStore>::ProjectionCheckpointStore;

    fn projection_checkpoint_store(&self) -> &Self::ProjectionCheckpointStore {
        DependOnProjectionCheckpointStore::projection_checkpoint_store(&self.db)
    }
}

impl DependOnAccountProjectionWriter for ProjectorTest {
    type AccountProjectionWriter =
        <PostgresDatabase as DependOnAccountProjectionWriter>::AccountProjectionWriter;

    fn account_projection_writer(&self) -> &Self::AccountProjectionWriter {
        DependOnAccountProjectionWriter::account_projection_writer(&self.db)
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
    sqlx::query(
        "INSERT INTO auth_accounts (id, host_id, client_id, version) VALUES ($1, $2, $3, 0)",
    )
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
    let updated = json!({ "type": "Updated", "is_bot": true });
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
    let updated = json!({ "type": "Updated", "is_bot": true });
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
