use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{Image, ImageId, ImageUrl};
use crate::KernelError;
use std::future::Future;

pub trait ImageRepository: Sync + Send + 'static {
    type Connection: Connection;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &ImageId,
    ) -> impl Future<Output = error_stack::Result<Option<Image>, KernelError>> + Send;

    /// Returns images matching the given IDs.
    /// Returns an empty vec if `ids` is empty.
    /// The order of results is not guaranteed to match the input order.
    fn find_by_ids(
        &self,
        executor: &mut Self::Connection,
        ids: &[ImageId],
    ) -> impl Future<Output = error_stack::Result<Vec<Image>, KernelError>> + Send;

    fn find_by_url(
        &self,
        executor: &mut Self::Connection,
        url: &ImageUrl,
    ) -> impl Future<Output = error_stack::Result<Option<Image>, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Connection,
        image: &Image,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Connection,
        image_id: &ImageId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnImageRepository: Sync + Send + DependOnDatabaseConnection {
    type ImageRepository: ImageRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn image_repository(&self) -> &Self::ImageRepository;
}
