use crate::dto::media::{UploadImageDto, UploadedImageDto};
use error_stack::Report;
use image::GenericImageView;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::repository::{DependOnImageRepository, ImageRepository};
use kernel::interfaces::storage::{DependOnImageStorage, ImageStorage};
use kernel::prelude::entity::{Image, ImageBlurHash, ImageHash, ImageId, ImageUrl};
use kernel::KernelError;
use sha2::Digest;
use std::future::Future;

pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

pub trait UploadImageUseCase: Sync + Send + DependOnImageRepository + DependOnImageStorage {
    fn upload_image(
        &self,
        dto: UploadImageDto,
    ) -> impl Future<Output = error_stack::Result<UploadedImageDto, KernelError>> + Send {
        async move {
            let extension = validate_upload(&dto)?;
            let processed = tokio::task::spawn_blocking(move || process_image(dto))
                .await
                .map_err(|error| {
                    Report::new(KernelError::Internal)
                        .attach_printable(format!("Image processing task failed: {error}"))
                })??;
            let id = ImageId::new(kernel::generate_id());
            let key = format!("images/{}.{}", id.as_ref(), extension);
            let stored = self
                .image_storage()
                .put(&key, &processed.content_type, &processed.bytes)
                .await?;
            let image = Image::new(
                id,
                ImageUrl::new(stored.url),
                ImageHash::new(processed.hash),
                ImageBlurHash::new(processed.blur_hash),
            );
            let mut executor = self.database_connection().connection().await?;
            self.image_repository()
                .create(&mut executor, &image)
                .await?;
            Ok(UploadedImageDto {
                id: image.id().as_ref().to_string(),
                url: image.url().as_ref().to_string(),
                hash: image.hash().as_ref().to_string(),
                blur_hash: image.blur_hash().as_ref().to_string(),
            })
        }
    }
}

impl<T> UploadImageUseCase for T where
    T: Sync + Send + DependOnImageRepository + DependOnImageStorage
{
}

struct ProcessedImage {
    content_type: String,
    bytes: Vec<u8>,
    hash: String,
    blur_hash: String,
}

fn validate_upload(dto: &UploadImageDto) -> error_stack::Result<&'static str, KernelError> {
    if dto.bytes.is_empty() {
        return Err(Report::new(KernelError::Validation)
            .attach_printable("Image file cannot be empty".to_string()));
    }
    if dto.bytes.len() > MAX_IMAGE_BYTES {
        return Err(
            Report::new(KernelError::Validation).attach_printable(format!(
                "Image file must not exceed {MAX_IMAGE_BYTES} bytes"
            )),
        );
    }
    match dto.content_type.as_str() {
        "image/png" => Ok("png"),
        "image/jpeg" => Ok("jpg"),
        "image/webp" => Ok("webp"),
        _ => Err(Report::new(KernelError::Validation).attach_printable(
            "Image MIME type must be image/png, image/jpeg, or image/webp".to_string(),
        )),
    }
}

fn process_image(dto: UploadImageDto) -> error_stack::Result<ProcessedImage, KernelError> {
    let image = image::load_from_memory(&dto.bytes).map_err(|error| {
        Report::new(KernelError::Validation)
            .attach_printable(format!("Image bytes are invalid: {error}"))
    })?;
    let (width, height) = image.dimensions();
    let thumbnail = image.thumbnail(64, 64).to_rgba8();
    let blur_hash = blurhash::encode(
        4,
        3,
        thumbnail.width(),
        thumbnail.height(),
        thumbnail.as_raw(),
    )
    .map_err(|error| {
        Report::new(KernelError::Internal)
            .attach_printable(format!("Failed to generate blurhash: {error}"))
    })?;
    if width == 0 || height == 0 {
        return Err(Report::new(KernelError::Validation)
            .attach_printable("Image dimensions must be non-zero".to_string()));
    }
    let hash = format!("{:x}", sha2::Sha256::digest(&dto.bytes));
    Ok(ProcessedImage {
        content_type: dto.content_type,
        bytes: dto.bytes,
        hash,
        blur_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsupported_mime_type() {
        let dto = UploadImageDto {
            content_type: "text/plain".to_string(),
            bytes: vec![1],
        };

        let result = validate_upload(&dto);

        assert!(result.is_err());
    }

    #[test]
    fn rejects_oversized_image() {
        let dto = UploadImageDto {
            content_type: "image/png".to_string(),
            bytes: vec![0; MAX_IMAGE_BYTES + 1],
        };

        let result = validate_upload(&dto);

        assert!(result.is_err());
    }
}
