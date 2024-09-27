use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use crate::entity::{Image, ImageId, ImageUrl};
use crate::KernelError;
use std::future::Future;

pub trait ImageQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &ImageId,
    ) -> impl Future<Output = error_stack::Result<Option<Image>, KernelError>> + Send;

    fn find_by_url(
        &self,
        transaction: &mut Self::Transaction,
        url: &ImageUrl,
    ) -> impl Future<Output = error_stack::Result<Option<Image>, KernelError>> + Send;
}

pub trait DependOnImageQuery: Sync + Send + DependOnDatabaseConnection {
    type ImageQuery: ImageQuery<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;

    fn image_query(&self) -> &Self::ImageQuery;
}
