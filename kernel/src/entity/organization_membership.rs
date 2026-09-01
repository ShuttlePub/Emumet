use crate::entity::{AccountId, CreatedAt};
use destructure::Destructure;
use serde::{Deserialize, Serialize};
use vodca::{Newln, References};

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrgRole {
    Owner,
    Admin,
    Member,
}

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationMembershipStatus {
    Pending,
    Active,
}

#[derive(
    Debug, Clone, Hash, Eq, PartialEq, References, Newln, Serialize, Deserialize, Destructure,
)]
pub struct OrganizationMembership {
    org_account_id: AccountId,
    member_account_id: AccountId,
    role: OrgRole,
    status: OrganizationMembershipStatus,
    invited_by: AccountId,
    created_at: CreatedAt<OrganizationMembership>,
}
