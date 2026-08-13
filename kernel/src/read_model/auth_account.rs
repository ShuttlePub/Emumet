use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{AuthAccount, AuthAccountClientId, AuthAccountId};
use crate::KernelError;
use std::future::Future;

pub trait AuthAccountReadModel: Sync + Send + 'static {
    type Connection: Connection;

    // Query operations (projection reads)
    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<Option<AuthAccount>, KernelError>> + Send;

    fn find_by_client_id(
        &self,
        executor: &mut Self::Connection,
        client_id: &AuthAccountClientId,
    ) -> impl Future<Output = error_stack::Result<Option<AuthAccount>, KernelError>> + Send;

    // Projection update operations (called by EventApplier pipeline)
    fn create(
        &self,
        executor: &mut Self::Connection,
        auth_account: &AuthAccount,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnAuthAccountReadModel: Sync + Send + DependOnDatabaseConnection {
    type AuthAccountReadModel: AuthAccountReadModel<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn auth_account_read_model(&self) -> &Self::AuthAccountReadModel;
}
