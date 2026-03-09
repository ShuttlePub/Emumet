use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::AuthHost;
use crate::KernelError;
use std::future::Future;

pub trait AuthHostModifier: Sync + Send + 'static {
    type Executor: Executor;

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

pub trait DependOnAuthHostModifier: Sync + Send + DependOnDatabaseConnection {
    type AuthHostModifier: AuthHostModifier<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn auth_host_modifier(&self) -> &Self::AuthHostModifier;
}
