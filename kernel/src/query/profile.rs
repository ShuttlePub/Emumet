use crate::database::{DependOnDatabaseConnection, Executor};
use crate::entity::{Profile, ProfileId};
use crate::KernelError;
use std::future::Future;

pub trait ProfileQuery: Sync + Send + 'static {
    type Executor: Executor;

    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<Option<Profile>, KernelError>> + Send;
}

pub trait DependOnProfileQuery: Sync + Send + DependOnDatabaseConnection {
    type ProfileQuery: ProfileQuery<
        Executor = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Executor,
    >;

    fn profile_query(&self) -> &Self::ProfileQuery;
}
