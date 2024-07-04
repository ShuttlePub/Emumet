use crate::KernelError;

pub trait Transaction {}

pub trait DatabaseConnection: Sync + Send + 'static {
    type Transaction: Transaction;
    async fn begin_transaction(&self) -> error_stack::Result<Self::Transaction, KernelError>;
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
