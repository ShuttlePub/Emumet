use crate::database::{DependOnDatabaseConnection, Executor};
use crate::entity::{AuthAccount, AuthAccountId};
use crate::KernelError;
use std::future::Future;

pub trait AuthAccountModifier: Sync + Send + 'static {
    type Executor: Executor;

    fn create(
        &self,
        executor: &mut Self::Executor,
        auth_account: &AuthAccount,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Executor,
        auth_account: &AuthAccount,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Executor,
        account_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnAuthAccountModifier: Sync + Send + DependOnDatabaseConnection {
    type AuthAccountModifier: AuthAccountModifier<
        Executor = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Executor,
    >;

    fn auth_account_modifier(&self) -> &Self::AuthAccountModifier;
}
