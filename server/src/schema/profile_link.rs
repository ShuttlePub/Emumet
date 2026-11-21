use kernel::interfaces::home_link::HomeLink;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterProfileLink {
    pub origin: String,
    pub trust_mode: i16,
}
#[derive(Serialize, ToSchema)]
pub struct ProfileLinkResponse {
    pub id: String,
    pub origin: String,
    pub trust_mode: i16,
    pub generation: i64,
    pub state: String,
    pub actor: String,
    pub created_at: String,
    pub unlinked_at: Option<String>,
    pub unlink_reason: Option<String>,
}
impl From<HomeLink> for ProfileLinkResponse {
    fn from(l: HomeLink) -> Self {
        Self {
            id: l.id,
            origin: l.origin,
            trust_mode: l.trust_mode,
            generation: l.generation,
            state: l.state,
            actor: l.actor,
            created_at: l
                .created_at
                .format(&time::format_description::well_known::Rfc3339)
                .expect("DB timestamp"),
            unlinked_at: l.unlinked_at.map(|t| {
                t.format(&time::format_description::well_known::Rfc3339)
                    .expect("DB timestamp")
            }),
            unlink_reason: l.unlink_reason,
        }
    }
}
#[derive(Serialize, ToSchema)]
pub struct ProfileLinkListResponse {
    pub items: Vec<ProfileLinkResponse>,
}
