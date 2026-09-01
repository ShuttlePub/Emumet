use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{AccountId, OrganizationMembership};
use crate::repository::{
    DependOnOrganizationMembershipRepository, OrganizationMembershipRepository,
};
use crate::KernelError;
use std::future::Future;

pub trait OrganizationMembershipQuery: Send + Sync + 'static {
    type Connection: Connection;

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
}

impl<T> OrganizationMembershipQuery for T
where
    T: DependOnOrganizationMembershipRepository + Send + Sync + 'static,
{
    type Connection = <<T as DependOnOrganizationMembershipRepository>::OrganizationMembershipRepository as OrganizationMembershipRepository>::Connection;

    async fn find(
        &self,
        executor: &mut Self::Connection,
        org_account_id: &AccountId,
        member_account_id: &AccountId,
    ) -> error_stack::Result<Option<OrganizationMembership>, KernelError> {
        self.organization_membership_repository()
            .find(executor, org_account_id, member_account_id)
            .await
    }

    async fn find_by_org(
        &self,
        executor: &mut Self::Connection,
        org_account_id: &AccountId,
    ) -> error_stack::Result<Vec<OrganizationMembership>, KernelError> {
        self.organization_membership_repository()
            .find_by_org(executor, org_account_id)
            .await
    }

    async fn find_by_member(
        &self,
        executor: &mut Self::Connection,
        member_account_id: &AccountId,
    ) -> error_stack::Result<Vec<OrganizationMembership>, KernelError> {
        self.organization_membership_repository()
            .find_by_member(executor, member_account_id)
            .await
    }
}

pub trait DependOnOrganizationMembershipQuery: DependOnDatabaseConnection + Send + Sync {
    type OrganizationMembershipQuery: OrganizationMembershipQuery<
        Connection = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn organization_membership_query(&self) -> &Self::OrganizationMembershipQuery;
}

impl<T> DependOnOrganizationMembershipQuery for T
where
    T: DependOnOrganizationMembershipRepository
        + DependOnDatabaseConnection
        + Send
        + Sync
        + 'static,
{
    type OrganizationMembershipQuery = Self;

    fn organization_membership_query(&self) -> &Self::OrganizationMembershipQuery {
        self
    }
}
