use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{RemoteAccount, RemoteAccountId};
use crate::KernelError;
use std::future::Future;

pub trait RemoteAccountModifier: Sync + Send + 'static {
    type Executor: Executor;

    fn create(
        &self,
        executor: &mut Self::Executor,
        account: &RemoteAccount,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Executor,
        account: &RemoteAccount,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Executor,
        account_id: &RemoteAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnRemoteAccountModifier: Sync + Send + DependOnDatabaseConnection {
    type RemoteAccountModifier: RemoteAccountModifier<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn remote_account_modifier(&self) -> &Self::RemoteAccountModifier;
}
