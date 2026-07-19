use crate::KernelError;
use std::future::Future;

/// Executorの取得を示すトレイト
///
/// 現状は何もないが、将来的にトランザクション時に使える機能を示す可能性を考えて用意している
pub trait Executor: Send {
    fn commit(self) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send
    where
        Self: Sized,
    {
        async { Ok(()) }
    }
}

pub trait DatabaseConnection: Sync + Send + 'static {
    type Executor: Executor;
    fn get_executor(
        &self,
    ) -> impl Future<Output = error_stack::Result<Self::Executor, KernelError>> + Send;

    fn get_transaction(
        &self,
    ) -> impl Future<Output = error_stack::Result<Self::Executor, KernelError>> + Send {
        self.get_executor()
    }
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
