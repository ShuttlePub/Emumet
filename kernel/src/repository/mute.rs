use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{Mute, MuteId, MuteTargetId};
use crate::KernelError;
use std::future::Future;

pub trait MuteRepository: Sync + Send + 'static {
    type Executor: Executor;

    fn find_mutes(
        &self,
        executor: &mut Self::Executor,
        source: &MuteTargetId,
    ) -> impl Future<Output = error_stack::Result<Vec<Mute>, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Executor,
        mute: &Mute,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Executor,
        mute_id: &MuteId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnMuteRepository: Sync + Send + DependOnDatabaseConnection {
    type MuteRepository: MuteRepository<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn mute_repository(&self) -> &Self::MuteRepository;
}
