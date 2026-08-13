use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{Mute, MuteId, MuteTargetId};
use crate::KernelError;
use std::future::Future;

pub trait MuteRepository: Sync + Send + 'static {
    type Connection: Connection;

    fn find_mutes(
        &self,
        executor: &mut Self::Connection,
        source: &MuteTargetId,
    ) -> impl Future<Output = error_stack::Result<Vec<Mute>, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Connection,
        mute: &Mute,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Connection,
        mute_id: &MuteId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnMuteRepository: Sync + Send + DependOnDatabaseConnection {
    type MuteRepository: MuteRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn mute_repository(&self) -> &Self::MuteRepository;
}
