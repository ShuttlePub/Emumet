use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::{Profile, ProfileId};
use crate::KernelError;
use std::future::Future;

pub trait ProfileQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<Option<Profile>, KernelError>> + Send;
}

pub trait DependOnProfileQuery: Sync + Send + DependOnDatabaseConnection {
    type ProfileQuery: ProfileQuery<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn profile_query(&self) -> &Self::ProfileQuery;
}
