use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{Image, ImageId, ImageUrl};
use crate::KernelError;
use std::future::Future;

pub trait ImageRepository: Sync + Send + 'static {
    type Executor: Executor;

    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &ImageId,
    ) -> impl Future<Output = error_stack::Result<Option<Image>, KernelError>> + Send;

    fn find_by_url(
        &self,
        executor: &mut Self::Executor,
        url: &ImageUrl,
    ) -> impl Future<Output = error_stack::Result<Option<Image>, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Executor,
        image: &Image,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Executor,
        image_id: &ImageId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnImageRepository: Sync + Send + DependOnDatabaseConnection {
    type ImageRepository: ImageRepository<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn image_repository(&self) -> &Self::ImageRepository;
}
