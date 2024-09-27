use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{Image, ImageId};
use crate::KernelError;
use std::future::Future;

pub trait ImageModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    fn create(
        &self,
        transaction: &mut Self::Transaction,
        image: &Image,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        transaction: &mut Self::Transaction,
        image_id: &ImageId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnImageModifier: Sync + Send + DependOnDatabaseConnection {
    type ImageModifier: ImageModifier<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn image_modifier(&self) -> &Self::ImageModifier;
}
