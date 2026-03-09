use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{Follow, FollowId};
use crate::KernelError;
use std::future::Future;

pub trait FollowModifier: Sync + Send + 'static {
    type Executor: Executor;

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

pub trait DependOnFollowModifier: Sync + Send + DependOnDatabaseConnection {
    type FollowModifier: FollowModifier<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn follow_modifier(&self) -> &Self::FollowModifier;
}
