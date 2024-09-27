use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{RemoteAccount, RemoteAccountAcct, RemoteAccountId, RemoteAccountUrl};
use crate::KernelError;
use std::future::Future;

pub trait RemoteAccountQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &RemoteAccountId,
    ) -> impl Future<Output = error_stack::Result<Option<RemoteAccount>, KernelError>> + Send;

    fn find_by_acct(
        &self,
        transaction: &mut Self::Transaction,
        acct: &RemoteAccountAcct,
    ) -> impl Future<Output = error_stack::Result<Option<RemoteAccount>, KernelError>> + Send;

    fn find_by_url(
        &self,
        transaction: &mut Self::Transaction,
        url: &RemoteAccountUrl,
    ) -> impl Future<Output = error_stack::Result<Option<RemoteAccount>, KernelError>> + Send;
}

pub trait DependOnRemoteAccountQuery: Sync + Send + DependOnDatabaseConnection {
    type RemoteAccountQuery: RemoteAccountQuery<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn remote_account_query(&self) -> &Self::RemoteAccountQuery;
}
