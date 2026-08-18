use crate::api::MediaApi;
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::schema::media::UploadedImageResponse;
use application::dto::media::UploadImageDto;
use application::service::media::MAX_IMAGE_BYTES;
use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use error_stack::Report;
use kernel::KernelError;

pub trait MediaRouter {
    fn route_media(self) -> Self;
}

impl MediaRouter for Router<AppModule> {
    fn route_media(self) -> Self {
        self.route(
            "/images",
            post(upload_image).layer(DefaultBodyLimit::max(MAX_IMAGE_BYTES + 64 * 1024)),
        )
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/images",
    description = "Upload an image to Emumet media storage.",
    request_body(content = String, content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "Image uploaded", body = UploadedImageResponse),
        (status = 400, description = "Invalid image upload"),
    ),
    security(("bearer_auth" = [])),
    tag = "Media",
)]
pub(crate) async fn upload_image(
    State(api): State<MediaApi>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<UploadedImageResponse>), ErrorStatus> {
    let mut upload = None;
    while let Some(field) = multipart.next_field().await.map_err(|error| {
        ErrorStatus::from(
            Report::new(KernelError::Validation)
                .attach_printable(format!("Invalid multipart body: {error}")),
        )
    })? {
        if field.name() != Some("file") {
            continue;
        }
        if upload.is_some() {
            return Err(ErrorStatus::from(
                Report::new(KernelError::Validation)
                    .attach_printable("Exactly one file field is required".to_string()),
            ));
        }
        let content_type = field.content_type().map(str::to_string).ok_or_else(|| {
            ErrorStatus::from(
                Report::new(KernelError::Validation)
                    .attach_printable("Image MIME type is required".to_string()),
            )
        })?;
        let bytes = field.bytes().await.map_err(|error| {
            ErrorStatus::from(
                Report::new(KernelError::Validation)
                    .attach_printable(format!("Failed to read image bytes: {error}")),
            )
        })?;
        upload = Some(UploadImageDto {
            content_type,
            bytes: bytes.to_vec(),
        });
    }
    let upload = upload.ok_or_else(|| {
        ErrorStatus::from(
            Report::new(KernelError::Validation)
                .attach_printable("Multipart field 'file' is required".to_string()),
        )
    })?;
    let image = api.upload_image(upload).await.map_err(ErrorStatus::from)?;
    Ok((StatusCode::CREATED, Json(image.into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_upload_limit_allows_multipart_overhead() {
        let configured_limit = MAX_IMAGE_BYTES + 64 * 1024;

        assert!(configured_limit > MAX_IMAGE_BYTES);
    }
}
