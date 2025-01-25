use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::{AuthAccount, AuthAccountClientId, AuthAccountId};
use crate::KernelError;
use std::future::Future;

pub trait AuthAccountQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<Option<AuthAccount>, KernelError>> + Send;

    fn find_by_client_id(
        &self,
        transaction: &mut Self::Transaction,
        client_id: &AuthAccountClientId,
    ) -> impl Future<Output = error_stack::Result<Option<AuthAccount>, KernelError>> + Send;
}

pub trait DependOnAuthAccountQuery: Sync + Send + DependOnDatabaseConnection {
    type AuthAccountQuery: AuthAccountQuery<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn auth_account_query(&self) -> &Self::AuthAccountQuery;
}
