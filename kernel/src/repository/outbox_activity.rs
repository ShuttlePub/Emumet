use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{AccountId, OutboxActivity, OutboxActivityId};
use crate::KernelError;
use std::future::Future;

pub trait OutboxActivityRepository: Sync + Send + 'static {
    type Connection: Connection;

    fn create(
        &self,
        executor: &mut Self::Connection,
        activity: &OutboxActivity,
    ) -> impl Future<Output = error_stack::Result<OutboxActivityId, KernelError>> + Send;

    fn find_by_account_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
        limit: usize,
        cursor: Option<i64>,
    ) -> impl Future<Output = error_stack::Result<Vec<OutboxActivity>, KernelError>> + Send;

    fn count_by_account_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<u64, KernelError>> + Send;

    fn find_pending_deliveries(
        &self,
        executor: &mut Self::Connection,
        limit: usize,
    ) -> impl Future<Output = error_stack::Result<Vec<OutboxActivity>, KernelError>> + Send;

    fn mark_delivered(
        &self,
        executor: &mut Self::Connection,
        id: &OutboxActivityId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn mark_delivery_attempt(
        &self,
        executor: &mut Self::Connection,
        id: &OutboxActivityId,
        error: Option<&str>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnOutboxActivityRepository: Sync + Send + DependOnDatabaseConnection {
    type OutboxActivityRepository: OutboxActivityRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn outbox_activity_repository(&self) -> &Self::OutboxActivityRepository;
}
