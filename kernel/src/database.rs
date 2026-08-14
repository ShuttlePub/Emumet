use crate::KernelError;
use std::future::Future;
use std::pin::Pin;

pub trait Connection: Send {}

pub trait Savepoint: Send {
    type Connection: Connection;
    fn commit(
        self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
    fn rollback(
        self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait Transaction: Send {
    type Connection: Connection;
    type Savepoint: Savepoint<Connection = Self::Connection>;
    fn connection(&mut self) -> &mut Self::Connection;
    fn commit(self) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    /// Open a savepoint on this transaction. Rolling the savepoint back leaves
    /// the transaction usable, so one failing aggregate cannot abort the whole
    /// batch.
    fn savepoint(
        &mut self,
    ) -> impl Future<Output = error_stack::Result<Self::Savepoint, KernelError>> + Send;
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
