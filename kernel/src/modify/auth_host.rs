use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::AuthHost;
use crate::KernelError;
use std::future::Future;

pub trait AuthHostModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    fn create(
        &self,
        transaction: &mut Self::Transaction,
        auth_host: &AuthHost,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        transaction: &mut Self::Transaction,
        auth_host: &AuthHost,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnAuthHostModifier: Sync + Send + DependOnDatabaseConnection {
    type AuthHostModifier: AuthHostModifier<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn auth_host_modifier(&self) -> &Self::AuthHostModifier;
}
