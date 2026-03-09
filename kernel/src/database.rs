use crate::KernelError;
use std::future::Future;

/// Databaseのトランザクション処理を示すトレイト
///
/// 現状は何もないが、将来的にトランザクション時に使える機能を示す可能性を考えて用意している
pub trait Executor: Send {}

pub trait DatabaseConnection: Sync + Send + 'static {
    type Executor: Executor;
    fn begin_transaction(
        &self,
    ) -> impl Future<Output = error_stack::Result<Self::Executor, KernelError>> + Send;
}

pub trait DependOnDatabaseConnection: Sync + Send {
    type DatabaseConnection: DatabaseConnection;
    fn database_connection(&self) -> &Self::DatabaseConnection;
}

impl<T> DependOnDatabaseConnection for T
where
    T: DatabaseConnection,
{
    type DatabaseConnection = T;
    fn database_connection(&self) -> &Self::DatabaseConnection {
        self
    }
}
