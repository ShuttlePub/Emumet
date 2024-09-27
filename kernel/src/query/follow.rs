use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{Follow, FollowTargetId};
use crate::KernelError;
use std::future::Future;

pub trait FollowQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    fn find_followings(
        &self,
        transaction: &mut Self::Transaction,
        source: &FollowTargetId,
    ) -> impl Future<Output = error_stack::Result<Vec<Follow>, KernelError>> + Send;

    fn find_followers(
        &self,
        transaction: &mut Self::Transaction,
        destination: &FollowTargetId,
    ) -> impl Future<Output = error_stack::Result<Vec<Follow>, KernelError>> + Send;
}

pub trait DependOnFollowQuery: Sync + Send + DependOnDatabaseConnection {
    type FollowQuery: FollowQuery<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn follow_query(&self) -> &Self::FollowQuery;
}
