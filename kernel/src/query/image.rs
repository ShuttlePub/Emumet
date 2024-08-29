use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{Image, ImageId, ImageUrl};
use crate::KernelError;

pub trait ImageQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &ImageId,
    ) -> error_stack::Result<Option<Image>, KernelError>;

    async fn find_by_url(
        &self,
        transaction: &mut Self::Transaction,
        url: &ImageUrl,
    ) -> error_stack::Result<Option<Image>, KernelError>;
}

pub trait DependOnImageQuery: Sync + Send + DependOnDatabaseConnection {
    type ImageQuery: ImageQuery<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn image_query(&self) -> &Self::ImageQuery;
}
