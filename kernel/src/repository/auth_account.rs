use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{AuthAccount, AuthAccountClientId, AuthAccountId, AuthHostId};
use crate::KernelError;
use std::future::Future;

pub trait AuthAccountRepository: Sync + Send + 'static {
    type Connection: Connection;

    fn find_or_create(
        &self,
        executor: &mut Self::Connection,
        host_id: &AuthHostId,
        client_id: &AuthAccountClientId,
    ) -> impl Future<Output = error_stack::Result<AuthAccount, KernelError>> + Send;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<Option<AuthAccount>, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Connection,
        auth_account: &AuthAccount,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnAuthAccountRepository: Sync + Send + DependOnDatabaseConnection {
    type AuthAccountRepository: AuthAccountRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn auth_account_repository(&self) -> &Self::AuthAccountRepository;
}
