use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::Profile;
use crate::KernelError;
use std::future::Future;

pub trait ProfileModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    fn create(
        &self,
        transaction: &mut Self::Transaction,
        profile: &Profile,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        transaction: &mut Self::Transaction,
        profile: &Profile,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnProfileModifier: Sync + Send + DependOnDatabaseConnection {
    type ProfileModifier: ProfileModifier<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn profile_modifier(&self) -> &Self::ProfileModifier;
}
