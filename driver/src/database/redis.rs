use crate::database::env;
use deadpool_redis::{Config, Pool, Runtime};
use error_stack::{Report, ResultExt};
use kernel::interfaces::database::{DatabaseConnection, Transaction};
use kernel::KernelError;
use std::ops::Deref;
use vodca::References;

// redis://127.0.0.1
const REDIS_URL: &str = "REDIS_URL";

// 127.0.0.1
const HOST: &str = "REDIS_HOST";

#[derive(Clone, References)]
pub struct RedisDatabase {
    pool: Pool,
}

impl RedisDatabase {
    pub fn new() -> error_stack::Result<RedisDatabase, KernelError> {
        let url = if let Some(env) = env(REDIS_URL)? {
            env
        } else {
            let host = env(HOST)?.ok_or_else(|| {
                Report::new(KernelError::Internal)
                    .attach_printable(format!("Failed to get env: {HOST}"))
            })?;
            format!("redis://{}", host)
        };
        let config = Config::from_url(&url);
        let pool = config
            .create_pool(Some(Runtime::Tokio1))
            .change_context_lazy(|| KernelError::Internal)?;
        Ok(Self { pool })
    }
}

pub struct RedisConnection(deadpool_redis::Connection);

impl Transaction for RedisConnection {}

impl Deref for RedisConnection {
    type Target = deadpool_redis::Connection;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DatabaseConnection for RedisDatabase {
    type Transaction = RedisConnection;

    async fn begin_transaction(&self) -> error_stack::Result<Self::Transaction, KernelError> {
        let pool = self
            .pool
            .get()
            .await
            .change_context_lazy(|| KernelError::Internal)?;
        Ok(RedisConnection(pool))
    }
}
