use crate::KernelError;
use std::future::Future;
use std::pin::Pin;

pub trait Connection: Send {}

pub trait Transaction: Send {
    type Connection: Connection;
    fn connection(&mut self) -> &mut Self::Connection;
    fn commit(self) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DatabaseConnection: Sync + Send + 'static {
    type Connection: Connection;
    fn connection(
        &self,
    ) -> impl Future<Output = error_stack::Result<Self::Connection, KernelError>> + Send;
}

pub trait TransactionalDatabaseConnection: DatabaseConnection {
    type Transaction: Transaction<Connection = Self::Connection>;
    fn get_transaction(
        &self,
    ) -> impl Future<Output = error_stack::Result<Self::Transaction, KernelError>> + Send;
}

pub trait TransactionManager: DatabaseConnection {
    fn transaction<'a, F, T>(
        &'a self,
        operation: F,
    ) -> Pin<Box<dyn Future<Output = error_stack::Result<T, KernelError>> + Send + 'a>>
    where
        F: for<'connection> FnOnce(
                &'connection mut Self::Connection,
            ) -> Pin<
                Box<dyn Future<Output = error_stack::Result<T, KernelError>> + Send + 'connection>,
            > + Send
            + 'a,
        T: Send + 'a;
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
