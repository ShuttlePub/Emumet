use crate::handler::AppModule;
use application::dto::media::{UploadImageDto, UploadedImageDto};
use application::service::media::UploadImageUseCase;
use axum::extract::FromRef;
use kernel::KernelError;
use std::sync::Arc;

#[derive(Clone)]
pub struct MediaApi {
    module: Arc<AppModule>,
}

impl MediaApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn upload_image(
        &self,
        dto: UploadImageDto,
    ) -> error_stack::Result<UploadedImageDto, KernelError> {
        self.module.upload_image(dto).await
    }
}

impl FromRef<AppModule> for MediaApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
