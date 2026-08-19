use crate::dto::media::{UploadImageDto, UploadedImageDto};
use error_stack::Report;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::repository::{DependOnImageRepository, ImageRepository};
use kernel::interfaces::storage::{DependOnImageStorage, ImageStorage};
use kernel::prelude::entity::{Image, ImageBlurHash, ImageHash, ImageId, ImageUrl};
use kernel::KernelError;
use sha2::Digest;
use std::future::Future;
use std::io::Cursor;

pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 4096;

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

fn sniffed_mime(bytes: &[u8]) -> Option<&'static str> {
    match image::guess_format(bytes).ok()? {
        image::ImageFormat::Png => Some("image/png"),
        image::ImageFormat::Jpeg => Some("image/jpeg"),
        image::ImageFormat::WebP => Some("image/webp"),
        _ => None,
    }
}

fn check_dimensions(width: u32, height: u32) -> error_stack::Result<(), KernelError> {
    if width == 0 || height == 0 {
        return Err(Report::new(KernelError::Validation)
            .attach_printable("Image dimensions must be non-zero".to_string()));
    }
    if width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(
            Report::new(KernelError::Validation).attach_printable(format!(
                "Image dimensions must not exceed {MAX_IMAGE_DIMENSION}px per side"
            )),
        );
    }
    Ok(())
}

fn process_image(dto: UploadImageDto) -> error_stack::Result<ProcessedImage, KernelError> {
    let actual_mime = sniffed_mime(&dto.bytes).ok_or_else(|| {
        Report::new(KernelError::Validation)
            .attach_printable("Image bytes are not a supported image format".to_string())
    })?;
    if actual_mime != dto.content_type {
        return Err(
            Report::new(KernelError::Validation).attach_printable(format!(
                "Declared MIME type {} does not match actual image format {actual_mime}",
                dto.content_type
            )),
        );
    }
    let (width, height) = image::ImageReader::new(Cursor::new(&dto.bytes))
        .with_guessed_format()
        .map_err(|error| {
            Report::new(KernelError::Validation)
                .attach_printable(format!("Failed to read image header: {error}"))
        })?
        .into_dimensions()
        .map_err(|error| {
            Report::new(KernelError::Validation)
                .attach_printable(format!("Failed to read image dimensions: {error}"))
        })?;
    check_dimensions(width, height)?;
    let image = image::load_from_memory(&dto.bytes).map_err(|error| {
        Report::new(KernelError::Validation)
            .attach_printable(format!("Image bytes are invalid: {error}"))
    })?;
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

    const PIXEL_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72,
        0xb6, 0x0d, 0x24, 0x00, 0x00, 0x00, 0x11, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x84, 0x19, 0x60, 0x0c, 0x00, 0x47, 0xca, 0x07, 0xf9, 0x67, 0x59,
        0x6e, 0xb7, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

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

    #[test]
    fn rejects_mime_spoofed_image() {
        let dto = UploadImageDto {
            content_type: "image/jpeg".to_string(),
            bytes: PIXEL_PNG.to_vec(),
        };

        let result = process_image(dto);

        assert!(result.is_err());
    }

    #[test]
    fn accepts_valid_png_and_generates_blurhash() {
        let dto = UploadImageDto {
            content_type: "image/png".to_string(),
            bytes: PIXEL_PNG.to_vec(),
        };

        let processed = process_image(dto).unwrap();

        assert_eq!(processed.content_type, "image/png");
        assert_eq!(processed.hash.len(), 64);
        assert!(!processed.blur_hash.is_empty());
    }

    #[test]
    fn rejects_zero_and_excessive_dimensions() {
        assert!(check_dimensions(0, 100).is_err());
        assert!(check_dimensions(100, 0).is_err());
        assert!(check_dimensions(MAX_IMAGE_DIMENSION + 1, 100).is_err());
        assert!(check_dimensions(100, MAX_IMAGE_DIMENSION + 1).is_err());
        assert!(check_dimensions(MAX_IMAGE_DIMENSION, MAX_IMAGE_DIMENSION).is_ok());
    }
}
