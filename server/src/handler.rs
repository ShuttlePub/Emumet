use crate::applier::ApplierContainer;
use driver::database::{PostgresDatabase, RedisDatabase};
use kernel::KernelError;
use std::sync::Arc;
use vodca::References;

#[derive(Clone, References)]
pub struct AppModule {
    handler: Arc<Handler>,
    applier_container: Arc<ApplierContainer>,
}

impl AppModule {
    pub async fn new() -> error_stack::Result<Self, KernelError> {
        let handler = Arc::new(Handler::init().await?);
        let applier_container = Arc::new(ApplierContainer::new(handler.clone()));
        Ok(Self {
            handler,
            applier_container,
        })
    }
}

#[derive(References)]
pub struct Handler {
    pgpool: PostgresDatabase,
    redis: RedisDatabase,
}

impl Handler {
    pub async fn init() -> error_stack::Result<Self, KernelError> {
        let pgpool = PostgresDatabase::new().await?;
        let redis = RedisDatabase::new()?;

        Ok(Self { pgpool, redis })
    }
}
