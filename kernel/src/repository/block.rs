use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{Block, BlockId, BlockTargetId};
use crate::KernelError;
use std::future::Future;

pub trait BlockRepository: Sync + Send + 'static {
    type Connection: Connection;

    fn find_blocks(
        &self,
        executor: &mut Self::Connection,
        source: &BlockTargetId,
    ) -> impl Future<Output = error_stack::Result<Vec<Block>, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Connection,
        block: &Block,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Connection,
        block_id: &BlockId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnBlockRepository: Sync + Send + DependOnDatabaseConnection {
    type BlockRepository: BlockRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn block_repository(&self) -> &Self::BlockRepository;
}
