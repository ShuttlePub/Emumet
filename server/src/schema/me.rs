use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct MeResponse {
    pub account_id: String,
    pub instance_roles: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionContextResponse {
    pub auth_account_id: String,
    pub instance_roles: Vec<String>,
    pub org_context: Option<OrganizationContextResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OrganizationContextResponse {
    pub org_account_id: String,
    pub role: String,
}
