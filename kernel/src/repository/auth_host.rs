use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{AuthHost, AuthHostId, AuthHostUrl};
use crate::KernelError;
use std::future::Future;

pub trait AuthHostRepository: Sync + Send + 'static {
    type Connection: Connection;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &AuthHostId,
    ) -> impl Future<Output = error_stack::Result<Option<AuthHost>, KernelError>> + Send;

    fn find_by_url(
        &self,
        executor: &mut Self::Connection,
        url: &AuthHostUrl,
    ) -> impl Future<Output = error_stack::Result<Option<AuthHost>, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Connection,
        auth_host: &AuthHost,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Connection,
        auth_host: &AuthHost,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnAuthHostRepository: Sync + Send + DependOnDatabaseConnection {
    type AuthHostRepository: AuthHostRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn auth_host_repository(&self) -> &Self::AuthHostRepository;
}
