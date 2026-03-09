use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{Image, ImageId};
use crate::KernelError;
use std::future::Future;

pub trait ImageModifier: Sync + Send + 'static {
    type Executor: Executor;

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

pub trait DependOnImageModifier: Sync + Send + DependOnDatabaseConnection {
    type ImageModifier: ImageModifier<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn image_modifier(&self) -> &Self::ImageModifier;
}
