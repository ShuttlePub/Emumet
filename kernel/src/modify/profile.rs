use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::Profile;
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
