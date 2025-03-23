use crate::database::env;
use deadpool_redis::{Config, Pool, Runtime};
use error_stack::{Report, ResultExt};
use kernel::KernelError;

// redis://127.0.0.1
const REDIS_URL: &str = "REDIS_URL";

// 127.0.0.1
const HOST: &str = "REDIS_HOST";

#[derive(Clone)]
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
