mod account;
mod account_event_store;
mod account_report;
mod account_report_event_store;
mod account_report_repository;
mod account_repository;
mod auth_account;
mod auth_host;
mod block;
mod domain_control;
mod follow;
mod home_link;
mod image;
mod instance_identity;
#[cfg(any(test, feature = "test-mode"))]
pub mod isolated_db;
mod metadata;
mod metadata_event_store;
mod metadata_repository;
mod mute;
mod organization_membership;
mod outbox_activity;
mod post_relay;
mod profile;
mod profile_block;
mod profile_delivery;
mod profile_event_store;
mod profile_follow;
mod profile_graph;
#[cfg(test)]
mod profile_graph_reset_tests;
#[cfg(test)]
mod profile_graph_test_support;
#[cfg(test)]
mod profile_graph_tests;
mod profile_inbound_activity;
mod profile_mute;
mod profile_relationship_lock;
mod profile_remote_actor_cache;
#[cfg(test)]
mod profile_remote_actor_cache_tests;
mod profile_repository;
mod profile_signing_key;
mod profile_transfer_request;
mod profile_transfer_request_event_store;
mod profile_transfer_request_repository;
mod projection;
mod registration_enforcement_probe;
mod registration_gate;
mod registration_protection;
mod remote_account;
mod signing_key;
#[cfg(test)]
mod transaction_manager_tests;
mod webhook_delivery;

use crate::database::env;
use crate::ConvertError;
use error_stack::{Report, ResultExt};
use kernel::interfaces::database::{
    Connection, DatabaseConnection, Savepoint, Transaction as DbTransaction, TransactionManager,
    TransactionalDatabaseConnection,
};
use kernel::KernelError;
use sqlx::pool::PoolConnection;
use sqlx::{Error, PgConnection, Pool, Postgres, Transaction};
use std::ops::{Deref, DerefMut};

/// 登録ゲート関連テーブルを TRUNCATE する実 DB テスト (E15a のゲートと E15w の配送) が共有する直列化 lock。
#[cfg(test)]
pub(crate) static REGISTRATION_GATE_DB_TEST_LOCK: tokio::sync::Mutex<()> =
    tokio::sync::Mutex::const_new(());

const POSTGRESQL: &str = "DATABASE_URL";

const HOST: &str = "DATABASE_HOST";
const PORT: &str = "DATABASE_PORT";
const USER: &str = "DATABASE_USER";
const PASSWORD: &str = "DATABASE_PASSWORD";
const DATABASE: &str = "DATABASE_NAME";

#[derive(Debug, Clone)]
pub struct PostgresDatabase {
    pool: Pool<Postgres>,
}

impl PostgresDatabase {
    pub async fn new() -> error_stack::Result<Self, KernelError> {
        let url = if let Some(env) = env(POSTGRESQL)? {
            env
        } else {
            let host = env(HOST)?.ok_or_else(|| Report::new(KernelError::Internal))?;
            let port = env(PORT)?.ok_or_else(|| Report::new(KernelError::Internal))?;
            let user = env(USER)?.ok_or_else(|| Report::new(KernelError::Internal))?;
            let password = env(PASSWORD)?.ok_or_else(|| Report::new(KernelError::Internal))?;
            let database = env(DATABASE)?.ok_or_else(|| Report::new(KernelError::Internal))?;
            format!(
                "postgresql://{}:{}@{}:{}/{}",
                user, password, host, port, database
            )
        };
        let pool = Pool::connect(&url).await.convert_error()?;
        {
            let mut connection = pool.acquire().await.convert_error()?;
            Self::migrate(&mut connection)
                .await
                .change_context_lazy(|| KernelError::Internal)?;
        }
        Ok(Self { pool })
    }

    /// Production startup migration path. Recheck all Profile usernames on EVERY attempt, not
    /// only when the earlier precheck migration is pending: another P4 migration may have failed
    /// after that precheck succeeded, and pre-cutover recovery can create more Profile events.
    /// The same SQL is used by the versioned guard, without changing published P4 checksums.
    pub(crate) async fn migrate(
        connection: &mut PgConnection,
    ) -> Result<(), sqlx::migrate::MigrateError> {
        let has_profile_usernames: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
             WHERE table_schema = 'public' AND table_name = 'profiles' AND column_name = 'username') \
             AND to_regclass('public.profile_events') IS NOT NULL",
        ).fetch_one(&mut *connection).await?;
        if has_profile_usernames {
            sqlx::query(include_str!(
                "../../../migrations/20261120000000_profile_federation_all_profiles_precheck.sql"
            ))
            .execute(&mut *connection)
            .await?;
        }
        sqlx::migrate!("../migrations").run(&mut *connection).await
    }

    pub fn from_pool(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }
}

enum PostgresConnectionInner {
    Connection(PoolConnection<Postgres>),
    Transaction(Transaction<'static, Postgres>),
    Vacant,
}

pub struct PostgresConnection(
    PostgresConnectionInner,
    Pool<Postgres>,
    std::sync::Arc<LifecycleBudget>,
);

struct LifecycleBudget {
    started_at: tokio::time::Instant,
    deadline: std::sync::OnceLock<tokio::time::Instant>,
    started: tokio::sync::Notify,
    /// Profile lifecycle gates already held by this transaction (see
    /// [`PostgresConnection::acquire_profile_gate`]).
    profile_gates: std::sync::Mutex<std::collections::BTreeSet<i64>>,
    account_gates: std::sync::Mutex<std::collections::BTreeSet<i64>>,
    /// A relationship gate was taken: no *new* Profile gate may follow (Profile -> relationship).
    relationship_gates_taken: std::sync::atomic::AtomicBool,
}
impl Default for LifecycleBudget {
    fn default() -> Self {
        Self {
            started_at: tokio::time::Instant::now(),
            deadline: std::sync::OnceLock::new(),
            started: tokio::sync::Notify::new(),
            profile_gates: std::sync::Mutex::new(std::collections::BTreeSet::new()),
            account_gates: std::sync::Mutex::new(std::collections::BTreeSet::new()),
            relationship_gates_taken: std::sync::atomic::AtomicBool::new(false),
        }
    }
}
impl LifecycleBudget {
    async fn expired(&self) {
        loop {
            let notified = self.started.notified();
            if let Some(deadline) = self.deadline.get() {
                tokio::time::sleep_until(*deadline).await;
                return;
            }
            notified.await;
        }
    }
}
impl PostgresConnection {
    /// Recorded only after PostgreSQL granted the transaction-scoped Account gate.
    pub(crate) fn note_account_gate(&self, id: i64) {
        self.2.account_gates.lock().unwrap().insert(id);
    }
    pub(crate) fn require_account_gate(&self, id: i64) -> error_stack::Result<(), KernelError> {
        if !self.is_transaction() || !self.2.account_gates.lock().unwrap().contains(&id) {
            return Err(Report::new(KernelError::Internal).attach_printable(format!(
                "Profile set write requires Account gate {id} in this transaction"
            )));
        }
        Ok(())
    }
    pub(crate) fn require_profile_gate(&self, id: i64) -> error_stack::Result<(), KernelError> {
        if self.is_transaction() && self.2.profile_gates.lock().unwrap().contains(&id) {
            Ok(())
        } else {
            Err(Report::new(KernelError::Internal)
                .attach_printable("post relay requires the Profile gate"))
        }
    }
    pub(crate) fn is_transaction(&self) -> bool {
        matches!(self.0, PostgresConnectionInner::Transaction(_))
    }
    /// Registers a Profile lifecycle gate this transaction is about to take. Returns `Ok(false)`
    /// when it is already held (re-entrant; nothing to acquire). A *new* gate is refused with
    /// `Concurrency` when it is not strictly above every Profile gate already held, or when a
    /// relationship gate was already taken: the shared order is Account gates -> Profile gates
    /// ascending -> relationship gates ascending -> links, so an out-of-order request would be a
    /// lock-order inversion. The caller rolls back and retries in a **new** transaction with the
    /// complete set (the set can only have grown concurrently).
    pub(crate) fn acquire_profile_gate(&self, id: i64) -> error_stack::Result<bool, KernelError> {
        let mut held = self.2.profile_gates.lock().unwrap();
        if held.contains(&id) {
            return Ok(false);
        }
        if self
            .2
            .relationship_gates_taken
            .load(std::sync::atomic::Ordering::SeqCst)
            || held.last().is_some_and(|max| *max > id)
        {
            return Err(
                Report::new(KernelError::Concurrency).attach_printable(format!(
                    "Profile gate {id} requested out of order; retry with a complete Profile set"
                )),
            );
        }
        held.insert(id);
        Ok(true)
    }
    pub(crate) fn note_relationship_gates_taken(&self) {
        self.2
            .relationship_gates_taken
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    pub(crate) fn lifecycle_budget(&self) -> error_stack::Result<(), KernelError> {
        if !self.is_transaction() {
            return Err(Report::new(KernelError::Internal)
                .attach_printable("lifecycle gate requires a transaction"));
        }
        self.2.deadline.get_or_init(|| {
            let deadline = self.2.started_at + std::time::Duration::from_secs(5);
            self.2.started.notify_one();
            deadline
        });
        Ok(())
    }
    pub(crate) async fn begin_atomic(&mut self) -> error_stack::Result<bool, KernelError> {
        if self.is_transaction() {
            return Ok(false);
        }
        let inner = std::mem::replace(&mut self.0, PostgresConnectionInner::Vacant);
        let PostgresConnectionInner::Connection(connection) = inner else {
            unreachable!()
        };
        self.0 = PostgresConnectionInner::Transaction(
            Transaction::begin(connection).await.convert_error()?,
        );
        self.2 = std::sync::Arc::new(LifecycleBudget::default());
        Ok(true)
    }
    pub(crate) async fn end_atomic(
        &mut self,
        success: bool,
    ) -> error_stack::Result<(), KernelError> {
        let inner = std::mem::replace(&mut self.0, PostgresConnectionInner::Vacant);
        let PostgresConnectionInner::Transaction(transaction) = inner else {
            unreachable!()
        };
        let result = if success {
            // Keep the original connection usable even when a timed commit fails.
            // The caller still receives the failure (and must never send on unknown commit).
            commit_with_budget(transaction, &self.2).await
        } else {
            transaction.rollback().await.convert_error()
        };
        self.0 = PostgresConnectionInner::Connection(self.1.acquire().await.convert_error()?);
        self.2 = std::sync::Arc::new(LifecycleBudget::default());
        result
    }
}

impl Connection for PostgresConnection {}

impl Deref for PostgresConnection {
    type Target = PgConnection;
    fn deref(&self) -> &Self::Target {
        match &self.0 {
            PostgresConnectionInner::Connection(connection) => connection,
            PostgresConnectionInner::Transaction(transaction) => transaction,
            PostgresConnectionInner::Vacant => {
                panic!("unusable connection after transaction failure")
            }
        }
    }
}

impl DerefMut for PostgresConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match &mut self.0 {
            PostgresConnectionInner::Connection(connection) => connection,
            PostgresConnectionInner::Transaction(transaction) => transaction,
            PostgresConnectionInner::Vacant => {
                panic!("unusable connection after transaction failure")
            }
        }
    }
}

pub struct PostgresTransaction(PostgresConnection);

pub struct PostgresSavepoint {
    account_gates: std::collections::BTreeSet<i64>,
    profile_gates: std::collections::BTreeSet<i64>,
    relationship_gates_taken: bool,
}

impl Savepoint for PostgresSavepoint {
    type Connection = PostgresConnection;

    async fn commit(self, executor: &mut Self::Connection) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query("RELEASE SAVEPOINT emumet_sp")
            .execute(con)
            .await
            .convert_error()?;
        Ok(())
    }

    async fn rollback(
        self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query("ROLLBACK TO SAVEPOINT emumet_sp")
            .execute(&mut *con)
            .await
            .convert_error()?;
        sqlx::query("RELEASE SAVEPOINT emumet_sp")
            .execute(&mut *con)
            .await
            .convert_error()?;
        *executor.2.account_gates.lock().unwrap() = self.account_gates;
        *executor.2.profile_gates.lock().unwrap() = self.profile_gates;
        executor.2.relationship_gates_taken.store(
            self.relationship_gates_taken,
            std::sync::atomic::Ordering::SeqCst,
        );
        Ok(())
    }
}

impl DbTransaction for PostgresTransaction {
    type Connection = PostgresConnection;
    type Savepoint = PostgresSavepoint;

    fn connection(&mut self) -> &mut Self::Connection {
        &mut self.0
    }

    async fn commit(self) -> error_stack::Result<(), KernelError> {
        match self.0 .0 {
            PostgresConnectionInner::Transaction(transaction) => {
                commit_with_budget(transaction, &self.0 .2).await
            }
            PostgresConnectionInner::Connection(_) | PostgresConnectionInner::Vacant => {
                unreachable!()
            }
        }
    }

    async fn savepoint(&mut self) -> error_stack::Result<Self::Savepoint, KernelError> {
        let con: &mut PgConnection = &mut self.0;
        sqlx::query("SAVEPOINT emumet_sp")
            .execute(con)
            .await
            .convert_error()?;
        Ok(PostgresSavepoint {
            account_gates: self.0 .2.account_gates.lock().unwrap().clone(),
            profile_gates: self.0 .2.profile_gates.lock().unwrap().clone(),
            relationship_gates_taken: self
                .0
                 .2
                .relationship_gates_taken
                .load(std::sync::atomic::Ordering::SeqCst),
        })
    }
}

async fn commit_with_budget(
    transaction: Transaction<'static, Postgres>,
    budget: &LifecycleBudget,
) -> error_stack::Result<(), KernelError> {
    if let Some(deadline) = budget.deadline.get() {
        tokio::time::timeout_at(*deadline, transaction.commit())
            .await
            .map_err(|_| {
                Report::new(KernelError::Internal)
                    .attach_printable("lifecycle commit exceeded 5s; do not send on unknown commit")
            })?
            .convert_error()
    } else {
        transaction.commit().await.convert_error()
    }
}

impl DatabaseConnection for PostgresDatabase {
    type Connection = PostgresConnection;
    async fn connection(&self) -> error_stack::Result<Self::Connection, KernelError> {
        let connection = self.pool.acquire().await.convert_error()?;
        Ok(PostgresConnection(
            PostgresConnectionInner::Connection(connection),
            self.pool.clone(),
            std::sync::Arc::new(LifecycleBudget::default()),
        ))
    }
}

impl TransactionalDatabaseConnection for PostgresDatabase {
    type Transaction = PostgresTransaction;

    async fn get_transaction(&self) -> error_stack::Result<Self::Transaction, KernelError> {
        let transaction = self.pool.begin().await.convert_error()?;
        Ok(PostgresTransaction(PostgresConnection(
            PostgresConnectionInner::Transaction(transaction),
            self.pool.clone(),
            std::sync::Arc::new(LifecycleBudget::default()),
        )))
    }
}

impl TransactionManager for PostgresDatabase {
    fn transaction<'a, F, T>(
        &'a self,
        operation: F,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = error_stack::Result<T, KernelError>> + Send + 'a>,
    >
    where
        F: for<'connection> FnOnce(
                &'connection mut Self::Connection,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = error_stack::Result<T, KernelError>>
                        + Send
                        + 'connection,
                >,
            > + Send
            + 'a,
        T: Send + 'a,
    {
        Box::pin(async move {
            let transaction = self.pool.begin().await.convert_error()?;
            let budget = std::sync::Arc::new(LifecycleBudget::default());
            let mut conn = PostgresConnection(
                PostgresConnectionInner::Transaction(transaction),
                self.pool.clone(),
                budget.clone(),
            );
            let result = tokio::select! {
                result = operation(&mut conn) => result,
                () = budget.expired() => Err(Report::new(KernelError::Internal).attach_printable("lifecycle transaction exceeded 5s")),
            };
            match result {
                Ok(result) => match conn.0 {
                    PostgresConnectionInner::Transaction(transaction) => {
                        commit_with_budget(transaction, &budget).await?;
                        Ok(result)
                    }
                    PostgresConnectionInner::Connection(_) | PostgresConnectionInner::Vacant => {
                        unreachable!()
                    }
                },
                Err(error) => {
                    match conn.0 {
                        PostgresConnectionInner::Transaction(transaction) => {
                            if let Err(rollback_error) = transaction.rollback().await {
                                tracing::error!(
                                    rollback_error = %rollback_error,
                                    "Transaction rollback failed; original error preserved"
                                );
                            }
                        }
                        PostgresConnectionInner::Connection(_)
                        | PostgresConnectionInner::Vacant => unreachable!(),
                    }
                    Err(error)
                }
            }
        })
    }
}

impl<T> ConvertError for Result<T, Error> {
    type Ok = T;
    fn convert_error(self) -> error_stack::Result<T, KernelError> {
        self.map_err(|error| match error {
            Error::PoolTimedOut => Report::from(error).change_context(KernelError::Timeout),
            _ => Report::from(error).change_context(KernelError::Internal),
        })
    }
}
