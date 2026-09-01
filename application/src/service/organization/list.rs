use crate::dto::organization::{MyOrganizationDto, OrganizationMemberDto, OrganizationSummaryDto};
use error_stack::Report;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::read_model::{
    AccountQuery, DependOnAccountQuery, DependOnOrganizationMembershipQuery,
    OrganizationMembershipQuery,
};
use kernel::prelude::entity::{
    Account, AccountKind, AuthAccountId, Nanoid, OrganizationMembershipStatus,
};
use kernel::KernelError;
use std::future::Future;

pub trait ListMyOrganizationsUseCase:
    Sync + Send + DependOnAccountQuery + DependOnOrganizationMembershipQuery
{
    fn list_my_organizations<'a>(
        &'a self,
        auth_id: &'a AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<MyOrganizationDto>, KernelError>> + Send + 'a
    {
        async move {
            let mut conn = self.database_connection().connection().await?;
            let accounts = self
                .account_query()
                .find_by_auth_id(&mut conn, auth_id)
                .await?;
            let mut result = Vec::new();
            for account in accounts
                .into_iter()
                .filter(|a| a.kind() == &AccountKind::Personal && a.deleted_at().is_none())
            {
                let memberships = self
                    .organization_membership_query()
                    .find_by_member(&mut conn, account.id())
                    .await?;
                for membership in memberships
                    .into_iter()
                    .filter(|m| m.status() == &OrganizationMembershipStatus::Active)
                {
                    if let Some(org) = self
                        .account_query()
                        .find_by_id(&mut conn, membership.org_account_id())
                        .await?
                        .filter(|a| a.kind() == &AccountKind::Organization)
                    {
                        result.push(MyOrganizationDto {
                            organization: OrganizationSummaryDto {
                                account_id: org.nanoid().as_ref().to_string(),
                                name: org.name().as_ref().to_string(),
                            },
                            role: *membership.role(),
                        });
                    }
                }
            }
            Ok(result)
        }
    }
}

impl<T> ListMyOrganizationsUseCase for T where
    T: Sync + Send + DependOnAccountQuery + DependOnOrganizationMembershipQuery
{
}

pub trait ListOrganizationMembersUseCase:
    Sync + Send + DependOnAccountQuery + DependOnOrganizationMembershipQuery
{
    fn list_organization_members<'a>(
        &'a self,
        auth_id: &'a AuthAccountId,
        org_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<Vec<OrganizationMemberDto>, KernelError>> + Send + 'a
    {
        async move {
            let mut conn = self.database_connection().connection().await?;
            let actor_accounts = self
                .account_query()
                .find_by_auth_id(&mut conn, auth_id)
                .await?;
            let org = self
                .account_query()
                .find_by_nanoid(&mut conn, &Nanoid::<Account>::new(org_nanoid))
                .await?
                .filter(|a| a.kind() == &AccountKind::Organization)
                .ok_or_else(|| Report::new(KernelError::NotFound))?;
            let mut visible = false;
            for account in actor_accounts
                .into_iter()
                .filter(|a| a.kind() == &AccountKind::Personal && a.deleted_at().is_none())
            {
                if self
                    .organization_membership_query()
                    .find(&mut conn, org.id(), account.id())
                    .await?
                    .is_some_and(|m| m.status() == &OrganizationMembershipStatus::Active)
                {
                    visible = true;
                    break;
                }
            }
            if !visible {
                return Err(Report::new(KernelError::PermissionDenied));
            }
            let memberships = self
                .organization_membership_query()
                .find_by_org(&mut conn, org.id())
                .await?;
            let mut result = Vec::new();
            for membership in memberships {
                let member = self
                    .account_query()
                    .find_by_id_unfiltered(&mut conn, membership.member_account_id())
                    .await?
                    .ok_or_else(|| Report::new(KernelError::NotFound))?;
                result.push(OrganizationMemberDto {
                    account_id: member.nanoid().as_ref().to_string(),
                    name: member.name().as_ref().to_string(),
                    role: *membership.role(),
                    status: *membership.status(),
                    invited_by: membership.invited_by().as_ref().to_string(),
                    created_at: *membership.created_at().as_ref(),
                });
            }
            Ok(result)
        }
    }
}

impl<T> ListOrganizationMembersUseCase for T where
    T: Sync + Send + DependOnAccountQuery + DependOnOrganizationMembershipQuery
{
}
