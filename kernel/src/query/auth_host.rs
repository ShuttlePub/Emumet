use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{AuthHost, AuthHostId, AuthHostUrl};
use crate::KernelError;
use std::future::Future;

pub trait AuthHostQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &AuthHostId,
    ) -> impl Future<Output = error_stack::Result<Option<AuthHost>, KernelError>> + Send;

    fn find_by_url(
        &self,
        transaction: &mut Self::Transaction,
        domain: &AuthHostUrl,
    ) -> impl Future<Output = error_stack::Result<Option<AuthHost>, KernelError>> + Send;
}

pub trait DependOnAuthHostQuery: Sync + Send + DependOnDatabaseConnection {
    type AuthHostQuery: AuthHostQuery<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn auth_host_query(&self) -> &Self::AuthHostQuery;
}
