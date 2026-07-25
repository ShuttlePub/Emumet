use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{Block, BlockId, BlockTargetId};
use crate::KernelError;
use std::future::Future;

pub trait BlockRepository: Sync + Send + 'static {
    type Executor: Executor;

    fn find_blocks(
        &self,
        executor: &mut Self::Executor,
        source: &BlockTargetId,
    ) -> impl Future<Output = error_stack::Result<Vec<Block>, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Executor,
        block: &Block,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Executor,
        block_id: &BlockId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnBlockRepository: Sync + Send + DependOnDatabaseConnection {
    type BlockRepository: BlockRepository<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn block_repository(&self) -> &Self::BlockRepository;
}
