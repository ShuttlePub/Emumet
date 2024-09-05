use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::{AccountId, CommandEnvelope, Profile, ProfileEvent};
use crate::KernelError;

pub trait ProfileModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn create(
        &self,
        transaction: &mut Self::Transaction,
        profile: &Profile,
    ) -> error_stack::Result<(), KernelError>;

    async fn update(
        &self,
        transaction: &mut Self::Transaction,
        profile: &Profile,
    ) -> error_stack::Result<(), KernelError>;
}

pub trait DependOnProfileModifier: Sync + Send + DependOnDatabaseConnection {
    type ProfileModifier: ProfileModifier<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn profile_modifier(&self) -> &Self::ProfileModifier;
}

pub trait ProfileEventModifier: 'static + Sync + Send {
    type Transaction: Transaction;

    async fn handle(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
        event: &CommandEnvelope<ProfileEvent, Profile>,
    ) -> error_stack::Result<(), KernelError>;
}

pub trait DependOnProfileEventModifier: Sync + Send + DependOnDatabaseConnection {
    type ProfileEventModifier: ProfileEventModifier<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn profile_event_modifier(&self) -> &Self::ProfileEventModifier;
}