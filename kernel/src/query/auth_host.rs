use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{AuthHost, AuthHostId, AuthHostUrl};
use crate::KernelError;
use std::future::Future;

pub trait AuthHostQuery: Sync + Send + 'static {
    type Executor: Executor;

    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &AuthHostId,
    ) -> impl Future<Output = error_stack::Result<Option<AuthHost>, KernelError>> + Send;

    fn find_by_url(
        &self,
        executor: &mut Self::Executor,
        domain: &AuthHostUrl,
    ) -> impl Future<Output = error_stack::Result<Option<AuthHost>, KernelError>> + Send;
}

pub trait DependOnAuthHostQuery: Sync + Send + DependOnDatabaseConnection {
    type AuthHostQuery: AuthHostQuery<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn auth_host_query(&self) -> &Self::AuthHostQuery;
}
