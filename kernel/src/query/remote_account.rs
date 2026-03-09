use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{RemoteAccount, RemoteAccountAcct, RemoteAccountId, RemoteAccountUrl};
use crate::KernelError;
use std::future::Future;

pub trait RemoteAccountQuery: Sync + Send + 'static {
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
}

pub trait DependOnRemoteAccountQuery: Sync + Send + DependOnDatabaseConnection {
    type RemoteAccountQuery: RemoteAccountQuery<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn remote_account_query(&self) -> &Self::RemoteAccountQuery;
}
