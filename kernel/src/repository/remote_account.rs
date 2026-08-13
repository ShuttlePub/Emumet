use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{RemoteAccount, RemoteAccountAcct, RemoteAccountId, RemoteAccountUrl};
use crate::KernelError;
use std::future::Future;

pub trait RemoteAccountRepository: Sync + Send + 'static {
    type Connection: Connection;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &RemoteAccountId,
    ) -> impl Future<Output = error_stack::Result<Option<RemoteAccount>, KernelError>> + Send;

    fn find_by_acct(
        &self,
        executor: &mut Self::Connection,
        acct: &RemoteAccountAcct,
    ) -> impl Future<Output = error_stack::Result<Option<RemoteAccount>, KernelError>> + Send;

    fn find_by_url(
        &self,
        executor: &mut Self::Connection,
        url: &RemoteAccountUrl,
    ) -> impl Future<Output = error_stack::Result<Option<RemoteAccount>, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Connection,
        account: &RemoteAccount,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Connection,
        account: &RemoteAccount,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Connection,
        account_id: &RemoteAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnRemoteAccountRepository: Sync + Send + DependOnDatabaseConnection {
    type RemoteAccountRepository: RemoteAccountRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn remote_account_repository(&self) -> &Self::RemoteAccountRepository;
}
