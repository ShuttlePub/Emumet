use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{AccountId, Profile, ProfileId};
use crate::KernelError;
use std::future::Future;

pub trait ProfileReadModel: Sync + Send + 'static {
    type Executor: Executor;

    // Query operations (projection reads)
    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<Option<Profile>, KernelError>> + Send;

    fn find_by_account_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<Profile>, KernelError>> + Send;

    fn find_by_account_ids(
        &self,
        executor: &mut Self::Executor,
        account_ids: &[AccountId],
    ) -> impl Future<Output = error_stack::Result<Vec<Profile>, KernelError>> + Send;

    // Projection update operations (called by EventApplier pipeline)
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

    fn delete(
        &self,
        executor: &mut Self::Executor,
        profile_id: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnProfileReadModel: Sync + Send + DependOnDatabaseConnection {
    type ProfileReadModel: ProfileReadModel<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn profile_read_model(&self) -> &Self::ProfileReadModel;
}
