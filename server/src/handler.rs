use driver::database::PostgresDatabase;
use kernel::KernelError;
use rikka_mq::define::redis::mq::RedisMessageQueue;
use std::sync::Arc;
use vodca::References;

#[derive(Clone, References)]
pub struct AppModule {
    handler: Arc<Handler>,
    worker: Arc<Worker>,
}

impl AppModule {
    pub async fn new() -> error_stack::Result<Self, KernelError> {
        let handler = Arc::new(Handler::init().await?);
        let worker = Arc::new(Worker::new(&handler));
        Ok(Self { handler, worker })
    }
}

#[derive(References)]
pub struct Handler {
    pgpool: PostgresDatabase,
}

impl Handler {
    pub async fn init() -> error_stack::Result<Self, KernelError> {
        let pgpool = PostgresDatabase::new().await?;

        Ok(Self { pgpool })
    }
}

#[derive(References)]
pub struct Worker {
    // command: RedisMessageQueue<Arc<Handler>, CommandOperation>,
}

impl Worker {
    pub fn new(handler: &Arc<Handler>) -> Self {
        // let command = init_command_worker(handler);
        // Self { command }
        Self
    }
}
