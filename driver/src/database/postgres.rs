mod account;
mod event;
mod follow;
mod image;
mod metadata;
mod profile;
mod remote_account;
mod stellar_account;
mod stellar_host;

use crate::database::env;
use crate::ConvertError;
use error_stack::{Report, ResultExt};
use kernel::interfaces::database::{DatabaseConnection, Transaction};
use kernel::KernelError;
use sqlx::pool::PoolConnection;
use sqlx::{Error, PgConnection, Pool, Postgres};
use std::ops::{Deref, DerefMut};
use uuid::Uuid;

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

#[derive(sqlx::FromRow)]
pub(in crate::database::postgres) struct VersionRow {
    pub version: Uuid,
}

#[derive(sqlx::FromRow)]
pub(in crate::database::postgres) struct CountRow {
    pub count: i64,
}

pub struct PostgresConnection(PoolConnection<Postgres>);

impl Transaction for PostgresConnection {}

impl Deref for PostgresConnection {
    type Target = PgConnection;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for PostgresConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl DatabaseConnection for PostgresDatabase {
    type Transaction = PostgresConnection;
    async fn begin_transaction(&self) -> error_stack::Result<Self::Transaction, KernelError> {
        let connection = self.pool.acquire().await.convert_error()?;
        Ok(PostgresConnection(connection))
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
