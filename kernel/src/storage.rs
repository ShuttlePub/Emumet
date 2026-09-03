use crate::entity::{AccountId, ImageId};
use crate::KernelError;
use std::future::Future;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    pub key: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileMediaCopyRequest {
    pub from_account_id: AccountId,
    pub to_account_id: AccountId,
    pub image_ids: Vec<ImageId>,
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

pub trait ProfileMediaCopyGateway: Send + Sync + 'static {
    fn request_copy(
        &self,
        request: ProfileMediaCopyRequest,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnProfileMediaCopyGateway: Send + Sync {
    type ProfileMediaCopyGateway: ProfileMediaCopyGateway;

    fn profile_media_copy_gateway(&self) -> &Self::ProfileMediaCopyGateway;
}
