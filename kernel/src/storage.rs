use crate::KernelError;
use std::future::Future;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    pub key: String,
    pub url: String,
}

pub trait ImageStorage: Send + Sync + 'static {
    fn put(
        &self,
        key: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> impl Future<Output = error_stack::Result<StoredObject, KernelError>> + Send;
}

pub trait DependOnImageStorage: Send + Sync {
    type ImageStorage: ImageStorage;

    fn image_storage(&self) -> &Self::ImageStorage;
}
