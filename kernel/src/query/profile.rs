use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::{EventEnvelope, EventVersion, Profile, ProfileEvent, ProfileId};
use crate::KernelError;

pub trait ProfileQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &ProfileId,
    ) -> error_stack::Result<Option<Profile>, KernelError>;
}

pub trait DependOnProfileQuery: Sync + Send + DependOnDatabaseConnection {
    type ProfileQuery: ProfileQuery<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn profile_query(&self) -> &Self::ProfileQuery;
}
