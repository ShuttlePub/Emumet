use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{AccountId, OrgRole, OrganizationMembership, OrganizationMembershipStatus};
use crate::KernelError;
use std::future::Future;

pub trait OrganizationMembershipRepository: Sync + Send + 'static {
    type Connection: Connection;

    fn create(
        &self,
        executor: &mut Self::Connection,
        membership: &OrganizationMembership,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn find(
        &self,
        executor: &mut Self::Connection,
        org_account_id: &AccountId,
        member_account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<OrganizationMembership>, KernelError>> + Send;

    fn find_by_org(
        &self,
        executor: &mut Self::Connection,
        org_account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<OrganizationMembership>, KernelError>> + Send;

    fn find_by_member(
        &self,
        executor: &mut Self::Connection,
        member_account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<OrganizationMembership>, KernelError>> + Send;

    fn update_role(
        &self,
        executor: &mut Self::Connection,
        org_account_id: &AccountId,
        member_account_id: &AccountId,
        role: OrgRole,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update_status(
        &self,
        executor: &mut Self::Connection,
        org_account_id: &AccountId,
        member_account_id: &AccountId,
        status: OrganizationMembershipStatus,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Connection,
        org_account_id: &AccountId,
        member_account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn count_active_owners(
        &self,
        executor: &mut Self::Connection,
        org_account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<i64, KernelError>> + Send;
}

pub trait DependOnOrganizationMembershipRepository:
    Sync + Send + DependOnDatabaseConnection
{
    type OrganizationMembershipRepository: OrganizationMembershipRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn organization_membership_repository(&self) -> &Self::OrganizationMembershipRepository;
}
