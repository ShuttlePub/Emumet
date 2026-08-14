use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{Follow, FollowId, FollowTargetId};
use crate::KernelError;
use std::future::Future;

pub trait FollowRepository: Sync + Send + 'static {
    type Connection: Connection;

    fn find_followings(
        &self,
        executor: &mut Self::Connection,
        source: &FollowTargetId,
    ) -> impl Future<Output = error_stack::Result<Vec<Follow>, KernelError>> + Send;

    fn find_followers(
        &self,
        executor: &mut Self::Connection,
        destination: &FollowTargetId,
    ) -> impl Future<Output = error_stack::Result<Vec<Follow>, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Connection,
        follow: &Follow,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Connection,
        follow: &Follow,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Connection,
        follow_id: &FollowId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn insert_if_absent(
        &self,
        executor: &mut Self::Connection,
        follow: &Follow,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    fn approve_follow_if_pending(
        &self,
        executor: &mut Self::Connection,
        source: &FollowTargetId,
        destination: &FollowTargetId,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    fn delete_if_exists(
        &self,
        executor: &mut Self::Connection,
        source: &FollowTargetId,
        destination: &FollowTargetId,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;
}

pub trait DependOnFollowRepository: Sync + Send + DependOnDatabaseConnection {
    type FollowRepository: FollowRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn follow_repository(&self) -> &Self::FollowRepository;
}
