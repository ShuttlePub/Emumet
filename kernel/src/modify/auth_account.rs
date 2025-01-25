use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::{AuthAccount, AuthAccountId};
use crate::KernelError;
use std::future::Future;

pub trait AuthAccountModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    fn create(
        &self,
        transaction: &mut Self::Transaction,
        auth_account: &AuthAccount,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        transaction: &mut Self::Transaction,
        auth_account: &AuthAccount,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnAuthAccountModifier: Sync + Send + DependOnDatabaseConnection {
    type AuthAccountModifier: AuthAccountModifier<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn auth_account_modifier(&self) -> &Self::AuthAccountModifier;
}
