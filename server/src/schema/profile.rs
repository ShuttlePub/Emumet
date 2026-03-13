use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct CreateProfileRequest {
    pub display_name: Option<String>,
    pub summary: Option<String>,
    pub icon: Option<Uuid>,
    pub banner: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProfileRequest {
    pub display_name: Option<String>,
    pub summary: Option<String>,
    pub icon: Option<Uuid>,
    pub banner: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct ProfileResponse {
    pub account_id: String,
    pub nanoid: String,
    pub display_name: Option<String>,
    pub summary: Option<String>,
    pub icon_id: Option<Uuid>,
    pub banner_id: Option<Uuid>,
}

impl From<application::transfer::profile::ProfileDto> for ProfileResponse {
    fn from(dto: application::transfer::profile::ProfileDto) -> Self {
        Self {
            account_id: dto.account_nanoid,
            nanoid: dto.nanoid,
            display_name: dto.display_name,
            summary: dto.summary,
            icon_id: dto.icon_id,
            banner_id: dto.banner_id,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct GetProfilesQuery {
    pub account_ids: String,
}
