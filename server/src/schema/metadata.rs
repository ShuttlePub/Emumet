use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateMetadataRequest {
    pub label: String,
    pub content: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateMetadataRequest {
    pub label: String,
    pub content: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MetadataResponse {
    pub account_id: String,
    pub nanoid: String,
    pub label: String,
    pub content: String,
}

impl From<application::transfer::metadata::MetadataDto> for MetadataResponse {
    fn from(dto: application::transfer::metadata::MetadataDto) -> Self {
        Self {
            account_id: dto.account_nanoid,
            nanoid: dto.nanoid,
            label: dto.label,
            content: dto.content,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GetMetadataQuery {
    pub account_ids: String,
}
