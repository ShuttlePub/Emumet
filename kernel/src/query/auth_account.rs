use crate::database::{DependOnDatabaseConnection, Executor};
use crate::entity::{AuthAccount, AuthAccountClientId, AuthAccountId};
use crate::KernelError;
use std::future::Future;

pub trait AuthAccountQuery: Sync + Send + 'static {
    type Executor: Executor;

    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<Option<AuthAccount>, KernelError>> + Send;

    fn find_by_client_id(
        &self,
        executor: &mut Self::Executor,
        client_id: &AuthAccountClientId,
    ) -> impl Future<Output = error_stack::Result<Option<AuthAccount>, KernelError>> + Send;
}

pub trait DependOnAuthAccountQuery: Sync + Send + DependOnDatabaseConnection {
    type AuthAccountQuery: AuthAccountQuery<
        Executor = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Executor,
    >;

    fn auth_account_query(&self) -> &Self::AuthAccountQuery;
}
