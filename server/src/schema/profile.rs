use kernel::prelude::entity::FieldAction;
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateProfileRequest {
    pub display_name: Option<String>,
    pub summary: Option<String>,
    pub icon_url: Option<String>,
    pub banner_url: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateProfileRequest {
    pub display_name: Option<String>,
    pub summary: Option<String>,
    /// Absent = no change, null = clear, string = set
    #[serde(default, deserialize_with = "deserialize_optional_nullable")]
    #[schema(nullable)]
    pub icon_url: Option<Option<String>>,
    /// Absent = no change, null = clear, string = set
    #[serde(default, deserialize_with = "deserialize_optional_nullable")]
    #[schema(nullable)]
    pub banner_url: Option<Option<String>>,
}

pub fn into_field_action<T>(value: Option<Option<T>>) -> FieldAction<T> {
    match value {
        None => FieldAction::Unchanged,
        Some(None) => FieldAction::Clear,
        Some(Some(v)) => FieldAction::Set(v),
    }
}

fn deserialize_optional_nullable<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileResponse {
    pub account_id: String,
    pub nanoid: String,
    pub display_name: Option<String>,
    pub summary: Option<String>,
    pub icon_url: Option<String>,
    pub banner_url: Option<String>,
}

impl From<application::transfer::profile::ProfileDto> for ProfileResponse {
    fn from(dto: application::transfer::profile::ProfileDto) -> Self {
        Self {
            account_id: dto.account_nanoid,
            nanoid: dto.nanoid,
            display_name: dto.display_name,
            summary: dto.summary,
            icon_url: dto.icon_url,
            banner_url: dto.banner_url,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GetProfilesQuery {
    pub account_ids: String,
}
