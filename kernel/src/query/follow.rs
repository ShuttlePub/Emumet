use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{Follow, FollowTargetId};
use crate::KernelError;
use std::future::Future;

pub trait FollowQuery: Sync + Send + 'static {
    type Executor: Executor;

    fn find_followings(
        &self,
        executor: &mut Self::Executor,
        source: &FollowTargetId,
    ) -> impl Future<Output = error_stack::Result<Vec<Follow>, KernelError>> + Send;

    fn find_followers(
        &self,
        executor: &mut Self::Executor,
        destination: &FollowTargetId,
    ) -> impl Future<Output = error_stack::Result<Vec<Follow>, KernelError>> + Send;
}

pub trait DependOnFollowQuery: Sync + Send + DependOnDatabaseConnection {
    type FollowQuery: FollowQuery<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn follow_query(&self) -> &Self::FollowQuery;
}
