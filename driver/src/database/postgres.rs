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
mod image;
#[cfg(any(test, feature = "test-mode"))]
pub mod isolated_db;
mod metadata;
mod metadata_event_store;
mod metadata_repository;
mod mute;
mod organization_membership;
mod outbox_activity;
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
}

pub struct PostgresConnection(PostgresConnectionInner);

impl Connection for PostgresConnection {}

impl Deref for PostgresConnection {
    type Target = PgConnection;
    fn deref(&self) -> &Self::Target {
        match &self.0 {
            PostgresConnectionInner::Connection(connection) => connection,
            PostgresConnectionInner::Transaction(transaction) => transaction,
        }
    }
}

impl DerefMut for PostgresConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match &mut self.0 {
            PostgresConnectionInner::Connection(connection) => connection,
            PostgresConnectionInner::Transaction(transaction) => transaction,
        }
    }
}

pub struct PostgresTransaction(PostgresConnection);

pub struct PostgresSavepoint;

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
            PostgresConnectionInner::Transaction(transaction) => transaction
                .commit()
                .await
                .change_context(KernelError::Internal),
            PostgresConnectionInner::Connection(_) => unreachable!(),
        }
    }

    async fn savepoint(&mut self) -> error_stack::Result<Self::Savepoint, KernelError> {
        let con: &mut PgConnection = &mut self.0;
        sqlx::query("SAVEPOINT emumet_sp")
            .execute(con)
            .await
            .convert_error()?;
        Ok(PostgresSavepoint)
    }
}

impl DatabaseConnection for PostgresDatabase {
    type Connection = PostgresConnection;
    async fn connection(&self) -> error_stack::Result<Self::Connection, KernelError> {
        let connection = self.pool.acquire().await.convert_error()?;
        Ok(PostgresConnection(PostgresConnectionInner::Connection(
            connection,
        )))
    }
}

impl TransactionalDatabaseConnection for PostgresDatabase {
    type Transaction = PostgresTransaction;

    async fn get_transaction(&self) -> error_stack::Result<Self::Transaction, KernelError> {
        let transaction = self.pool.begin().await.convert_error()?;
        Ok(PostgresTransaction(PostgresConnection(
            PostgresConnectionInner::Transaction(transaction),
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
            let mut conn = PostgresConnection(PostgresConnectionInner::Transaction(transaction));
            match operation(&mut conn).await {
                Ok(result) => match conn.0 {
                    PostgresConnectionInner::Transaction(transaction) => {
                        transaction
                            .commit()
                            .await
                            .change_context(KernelError::Internal)?;
                        Ok(result)
                    }
                    PostgresConnectionInner::Connection(_) => unreachable!(),
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
                        PostgresConnectionInner::Connection(_) => unreachable!(),
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
