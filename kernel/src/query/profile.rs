use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::{AccountId, EventEnvelope, EventVersion, Profile, ProfileEvent};
use crate::KernelError;

pub trait ProfileQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
    ) -> error_stack::Result<Option<Profile>, KernelError>;
}

pub trait DependOnProfileQuery: Sync + Send {
    type ProfileQuery: ProfileQuery<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn profile_query(&self) -> &Self::ProfileQuery;
}

pub trait ProfileEventQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &AccountId,
        since: Option<&EventVersion<Profile>>,
    ) -> error_stack::Result<Vec<EventEnvelope<ProfileEvent, Profile>>, KernelError>;
}

pub trait DependOnProfileEventQuery: Sync + Send + DependOnDatabaseConnection {
    type ProfileEventQuery: ProfileEventQuery<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn profile_event_query(&self) -> &Self::ProfileEventQuery;
}