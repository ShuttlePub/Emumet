use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{RemoteAccount, RemoteAccountAcct, RemoteAccountId, RemoteAccountUrl};
use crate::KernelError;
use std::future::Future;

pub trait RemoteAccountRepository: Sync + Send + 'static {
    type Executor: Executor;

    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &RemoteAccountId,
    ) -> impl Future<Output = error_stack::Result<Option<RemoteAccount>, KernelError>> + Send;

    fn find_by_acct(
        &self,
        executor: &mut Self::Executor,
        acct: &RemoteAccountAcct,
    ) -> impl Future<Output = error_stack::Result<Option<RemoteAccount>, KernelError>> + Send;

    fn find_by_url(
        &self,
        executor: &mut Self::Executor,
        url: &RemoteAccountUrl,
    ) -> impl Future<Output = error_stack::Result<Option<RemoteAccount>, KernelError>> + Send;

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

pub trait DependOnRemoteAccountRepository: Sync + Send + DependOnDatabaseConnection {
    type RemoteAccountRepository: RemoteAccountRepository<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn remote_account_repository(&self) -> &Self::RemoteAccountRepository;
}
