use crate::KernelError;
use std::future::Future;

/// Connectionの取得を示すトレイト
///
/// 現状は何もないが、将来的にトランザクション時に使える機能を示す可能性を考えて用意している
pub trait Connection: Send {
    fn commit(self) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send
    where
        Self: Sized,
    {
        async { Ok(()) }
    }
}

pub trait DatabaseConnection: Sync + Send + 'static {
    type Connection: Connection;
    fn connection(
        &self,
    ) -> impl Future<Output = error_stack::Result<Self::Connection, KernelError>> + Send;

    fn get_transaction(
        &self,
    ) -> impl Future<Output = error_stack::Result<Self::Connection, KernelError>> + Send {
        self.connection()
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
