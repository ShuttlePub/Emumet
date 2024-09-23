mod account;
mod event;
mod follow;
mod image;
mod metadata;
mod profile;
mod remote_account;
mod stellar_account;

use crate::database::env;
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, Transaction};
use kernel::KernelError;
use sqlx::pool::PoolConnection;
use sqlx::{Error, PgConnection, Pool, Postgres};
use std::ops::{Deref, DerefMut};
use uuid::Uuid;

const POSTGRESQL: &str = "postgresql";

#[derive(Debug, Clone)]
pub struct PostgresDatabase {
    pool: Pool<Postgres>,
}

impl PostgresDatabase {
    pub async fn new() -> error_stack::Result<Self, KernelError> {
        let url = env(POSTGRESQL)?;
        let pool = Pool::connect(&url).await.convert_error()?;
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
