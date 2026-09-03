use application::dto::profile_transfer::ProfileTransferRequestDto;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct CreateProfileTransferRequest {
    pub org_account_nanoid: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileTransferRequestResponse {
    pub nanoid: String,
    pub profile_nanoid: String,
    pub org_account_nanoid: String,
    pub status: String,
}

impl From<&ProfileTransferRequestDto> for ProfileTransferRequestResponse {
    fn from(dto: &ProfileTransferRequestDto) -> Self {
        Self {
            nanoid: dto.nanoid.clone(),
            profile_nanoid: dto.profile_nanoid.clone(),
            org_account_nanoid: dto.org_account_nanoid.clone(),
            status: dto.status.clone(),
        }
    }
}
