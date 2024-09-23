use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{Follow, FollowId};
use crate::KernelError;

pub trait FollowModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn create(
        &self,
        transaction: &mut Self::Transaction,
        follow: &Follow,
    ) -> error_stack::Result<(), KernelError>;

    async fn update(
        &self,
        transaction: &mut Self::Transaction,
        follow: &Follow,
    ) -> error_stack::Result<(), KernelError>;

    async fn delete(
        &self,
        transaction: &mut Self::Transaction,
        follow_id: &FollowId,
    ) -> error_stack::Result<(), KernelError>;
}

pub trait DependOnFollowModifier: Sync + Send + DependOnDatabaseConnection {
    type FollowModifier: FollowModifier<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn follow_modifier(&self) -> &Self::FollowModifier;
}
