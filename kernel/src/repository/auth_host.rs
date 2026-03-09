use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{AuthHost, AuthHostId, AuthHostUrl};
use crate::KernelError;
use std::future::Future;

pub trait AuthHostRepository: Sync + Send + 'static {
    type Executor: Executor;

    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &AuthHostId,
    ) -> impl Future<Output = error_stack::Result<Option<AuthHost>, KernelError>> + Send;

    fn find_by_url(
        &self,
        executor: &mut Self::Executor,
        url: &AuthHostUrl,
    ) -> impl Future<Output = error_stack::Result<Option<AuthHost>, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Executor,
        auth_host: &AuthHost,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Executor,
        auth_host: &AuthHost,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnAuthHostRepository: Sync + Send + DependOnDatabaseConnection {
    type AuthHostRepository: AuthHostRepository<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn auth_host_repository(&self) -> &Self::AuthHostRepository;
}
