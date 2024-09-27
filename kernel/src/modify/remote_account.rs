use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{RemoteAccount, RemoteAccountId};
use crate::KernelError;
use std::future::Future;

pub trait RemoteAccountModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    fn create(
        &self,
        transaction: &mut Self::Transaction,
        account: &RemoteAccount,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        transaction: &mut Self::Transaction,
        account: &RemoteAccount,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &RemoteAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnRemoteAccountModifier: Sync + Send + DependOnDatabaseConnection {
    type RemoteAccountModifier: RemoteAccountModifier<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn remote_account_modifier(&self) -> &Self::RemoteAccountModifier;
}
