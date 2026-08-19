use application::dto::media::UploadedImageDto;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct UploadedImageResponse {
    pub id: String,
    pub url: String,
    pub hash: String,
    pub blur_hash: String,
}

impl From<UploadedImageDto> for UploadedImageResponse {
    fn from(dto: UploadedImageDto) -> Self {
        Self {
            id: dto.id,
            url: dto.url,
            hash: dto.hash,
            blur_hash: dto.blur_hash,
        }
    }
}
