use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{RemoteAccount, RemoteAccountAcct, RemoteAccountId, RemoteAccountUrl};
use crate::KernelError;

pub trait RemoteAccountQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &RemoteAccountId,
    ) -> error_stack::Result<Option<RemoteAccount>, KernelError>;

    async fn find_by_acct(
        &self,
        transaction: &mut Self::Transaction,
        acct: &RemoteAccountAcct,
    ) -> error_stack::Result<Option<RemoteAccount>, KernelError>;

    async fn find_by_url(
        &self,
        transaction: &mut Self::Transaction,
        url: &RemoteAccountUrl,
    ) -> error_stack::Result<Option<RemoteAccount>, KernelError>;
}

pub trait DependOnRemoteAccountQuery: Sync + Send + DependOnDatabaseConnection {
    type RemoteAccountQuery: RemoteAccountQuery<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn remote_account_query(&self) -> &Self::RemoteAccountQuery;
}
