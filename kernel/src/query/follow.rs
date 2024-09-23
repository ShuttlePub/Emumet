use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{Follow, FollowTargetId};
use crate::KernelError;

pub trait FollowQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn find_followers(
        &self,
        transaction: &mut Self::Transaction,
        target_id: &FollowTargetId,
    ) -> error_stack::Result<Vec<Follow>, KernelError>;

    async fn find_followings(
        &self,
        transaction: &mut Self::Transaction,
        target_id: &FollowTargetId,
    ) -> error_stack::Result<Vec<Follow>, KernelError>;
}

pub trait DependOnFollowQuery: Sync + Send + DependOnDatabaseConnection {
    type FollowQuery: FollowQuery<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn follow_query(&self) -> &Self::FollowQuery;
}
