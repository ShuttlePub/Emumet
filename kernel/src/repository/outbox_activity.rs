use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{AccountId, OutboxActivity};
use crate::KernelError;
use std::future::Future;

pub trait OutboxActivityRepository: Sync + Send + 'static {
    type Executor: Executor;

    fn create(
        &self,
        executor: &mut Self::Executor,
        activity: &OutboxActivity,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn find_by_account_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
        limit: usize,
        cursor: Option<i64>,
    ) -> impl Future<Output = error_stack::Result<Vec<OutboxActivity>, KernelError>> + Send;

    fn count_by_account_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<u64, KernelError>> + Send;
}

pub trait DependOnOutboxActivityRepository: Sync + Send + DependOnDatabaseConnection {
    type OutboxActivityRepository: OutboxActivityRepository<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn outbox_activity_repository(&self) -> &Self::OutboxActivityRepository;
}
