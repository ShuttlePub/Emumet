use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{Follow, FollowId, FollowTargetId};
use crate::KernelError;
use std::future::Future;

pub trait FollowRepository: Sync + Send + 'static {
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

    fn create(
        &self,
        executor: &mut Self::Executor,
        follow: &Follow,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Executor,
        follow: &Follow,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Executor,
        follow_id: &FollowId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnFollowRepository: Sync + Send + DependOnDatabaseConnection {
    type FollowRepository: FollowRepository<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn follow_repository(&self) -> &Self::FollowRepository;
}
