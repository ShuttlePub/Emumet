use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{Image, ImageId};
use crate::KernelError;

pub trait ImageModifier: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn create(
        &self,
        transaction: &mut Self::Transaction,
        image: &Image,
    ) -> error_stack::Result<(), KernelError>;

    async fn delete(
        &self,
        transaction: &mut Self::Transaction,
        image_id: &ImageId,
    ) -> error_stack::Result<(), KernelError>;
}

pub trait DependOnImageModifier: Sync + Send + DependOnDatabaseConnection {
    type ImageModifier: ImageModifier<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn image_modifier(&self) -> &Self::ImageModifier;
}
