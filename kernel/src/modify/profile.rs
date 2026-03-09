use crate::database::{DependOnDatabaseConnection, Executor};
use crate::entity::Profile;
use crate::KernelError;
use std::future::Future;

pub trait ProfileModifier: Sync + Send + 'static {
    type Executor: Executor;

    fn create(
        &self,
        executor: &mut Self::Executor,
        profile: &Profile,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Executor,
        profile: &Profile,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnProfileModifier: Sync + Send + DependOnDatabaseConnection {
    type ProfileModifier: ProfileModifier<
        Executor = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Executor,
    >;

    fn profile_modifier(&self) -> &Self::ProfileModifier;
}
