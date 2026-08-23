use kernel::interfaces::database::{Connection, DatabaseConnection, TransactionManager};
use kernel::KernelError;
use std::future::Future;
use std::pin::Pin;

#[derive(Clone)]
pub struct MockConnection;

impl Connection for MockConnection {}

#[derive(Clone)]
pub struct MockDatabaseConnection;

impl DatabaseConnection for MockDatabaseConnection {
    type Connection = MockConnection;

    async fn connection(&self) -> error_stack::Result<Self::Connection, KernelError> {
        Ok(MockConnection)
    }
}

impl TransactionManager for MockDatabaseConnection {
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
        T: Send + 'a,
    {
        Box::pin(async move {
            let mut connection = self.connection().await?;
            operation(&mut connection).await
        })
    }
}
