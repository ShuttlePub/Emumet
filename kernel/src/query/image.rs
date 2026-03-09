use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{Image, ImageId, ImageUrl};
use crate::KernelError;
use std::future::Future;

pub trait ImageQuery: Sync + Send + 'static {
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
}

pub trait DependOnImageQuery: Sync + Send + DependOnDatabaseConnection {
    type ImageQuery: ImageQuery<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn image_query(&self) -> &Self::ImageQuery;
}
