use application::dto::organization::{
    CreateOrganizationDto, MyOrganizationDto, OrganizationMemberDto, OrganizationSummaryDto,
};
use kernel::prelude::entity::{OrgRole, OrganizationMembershipStatus};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateOrganizationRequest {
    pub name: String,
}

impl CreateOrganizationRequest {
    pub fn into_dto(self) -> CreateOrganizationDto {
        CreateOrganizationDto { name: self.name }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationRole {
    Owner,
    Admin,
    Member,
}

impl From<OrganizationRole> for OrgRole {
    fn from(value: OrganizationRole) -> Self {
        match value {
            OrganizationRole::Owner => OrgRole::Owner,
            OrganizationRole::Admin => OrgRole::Admin,
            OrganizationRole::Member => OrgRole::Member,
        }
    }
}

impl From<OrgRole> for OrganizationRole {
    fn from(value: OrgRole) -> Self {
        match value {
            OrgRole::Owner => OrganizationRole::Owner,
            OrgRole::Admin => OrganizationRole::Admin,
            OrgRole::Member => OrganizationRole::Member,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MembershipStatus {
    Pending,
    Active,
}

impl From<OrganizationMembershipStatus> for MembershipStatus {
    fn from(value: OrganizationMembershipStatus) -> Self {
        match value {
            OrganizationMembershipStatus::Pending => MembershipStatus::Pending,
            OrganizationMembershipStatus::Active => MembershipStatus::Active,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct InviteMemberRequest {
    pub account_id: String,
    pub role: OrganizationRole,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ChangeRoleRequest {
    pub role: OrganizationRole,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OrganizationResponse {
    pub id: String,
    pub name: String,
}

impl From<OrganizationSummaryDto> for OrganizationResponse {
    fn from(value: OrganizationSummaryDto) -> Self {
        Self {
            id: value.account_id,
            name: value.name,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MyOrganizationResponse {
    pub organization: OrganizationResponse,
    pub role: OrganizationRole,
}

impl From<MyOrganizationDto> for MyOrganizationResponse {
    fn from(value: MyOrganizationDto) -> Self {
        Self {
            organization: value.organization.into(),
            role: value.role.into(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MyOrganizationsResponse {
    pub items: Vec<MyOrganizationResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OrganizationMemberResponse {
    pub account_id: String,
    pub name: String,
    pub role: OrganizationRole,
    pub status: MembershipStatus,
    pub invited_by: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl From<OrganizationMemberDto> for OrganizationMemberResponse {
    fn from(value: OrganizationMemberDto) -> Self {
        Self {
            account_id: value.account_id,
            name: value.name,
            role: value.role.into(),
            status: value.status.into(),
            invited_by: value.invited_by,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OrganizationMembersResponse {
    pub items: Vec<OrganizationMemberResponse>,
}
