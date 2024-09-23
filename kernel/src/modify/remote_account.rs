use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{RemoteAccount, RemoteAccountId};
use crate::KernelError;

pub trait RemoteAccountModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn create(
        &self,
        transaction: &mut Self::Transaction,
        account: &RemoteAccount,
    ) -> error_stack::Result<(), KernelError>;

    async fn update(
        &self,
        transaction: &mut Self::Transaction,
        account: &RemoteAccount,
    ) -> error_stack::Result<(), KernelError>;

    async fn delete(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &RemoteAccountId,
    ) -> error_stack::Result<(), KernelError>;
}

pub trait DependOnRemoteAccountModifier: Sync + Send + DependOnDatabaseConnection {
    type RemoteAccountModifier: RemoteAccountModifier<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn remote_account_modifier(&self) -> &Self::RemoteAccountModifier;
}
