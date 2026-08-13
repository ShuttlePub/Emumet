mod account;
mod account_event_store;
mod account_repository;
mod auth_account;
mod auth_account_event_store;
mod auth_host;
mod block;
mod follow;
mod image;
mod metadata;
mod metadata_event_store;
mod mute;
mod outbox_activity;
mod profile;
mod profile_event_store;
mod remote_account;
mod signing_key;
#[cfg(test)]
mod transaction_manager_tests;

use crate::database::env;
use crate::ConvertError;
use error_stack::{Report, ResultExt};
use kernel::interfaces::database::{
    Connection, DatabaseConnection, Transaction as DbTransaction, TransactionManager,
    TransactionalDatabaseConnection,
};
use kernel::KernelError;
use sqlx::pool::PoolConnection;
use sqlx::{Error, PgConnection, Pool, Postgres, Transaction};
use std::ops::{Deref, DerefMut};

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
        sqlx::migrate!("../migrations")
            .run(&pool)
            .await
            .change_context_lazy(|| KernelError::Internal)?;
        Ok(Self { pool })
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

impl DbTransaction for PostgresTransaction {
    type Connection = PostgresConnection;

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
