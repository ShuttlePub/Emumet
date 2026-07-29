use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct MeResponse {
    pub account_id: String,
    pub instance_roles: Vec<String>,
}
