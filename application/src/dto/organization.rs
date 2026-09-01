use kernel::prelude::entity::{OrgRole, OrganizationMembershipStatus};
use time::OffsetDateTime;

#[derive(Debug)]
pub struct CreateOrganizationDto {
    pub name: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct OrganizationSummaryDto {
    pub account_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MyOrganizationDto {
    pub organization: OrganizationSummaryDto,
    pub role: OrgRole,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct OrganizationMemberDto {
    pub account_id: String,
    pub name: String,
    pub role: OrgRole,
    pub status: OrganizationMembershipStatus,
    pub invited_by: String,
    pub created_at: OffsetDateTime,
}
