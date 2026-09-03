#![allow(clippy::manual_async_fn)]

mod cancel;
mod request;
mod respond;

#[cfg(test)]
mod test_support;

pub use cancel::CancelProfileTransferRequestUseCase;
pub use request::RequestProfileTransferUseCase;
pub use respond::{AcceptProfileTransferRequestUseCase, RejectProfileTransferRequestUseCase};

use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::read_model::{AccountQuery, DependOnAccountQuery};
use kernel::interfaces::repository::{
    DependOnOrganizationMembershipRepository, OrganizationMembershipRepository,
};
use kernel::prelude::entity::{
    Account, AccountId, AccountKind, AuthAccountId, OrgRole, OrganizationMembershipStatus,
};
use kernel::KernelError;

async fn personal_accounts_of<T>(
    deps: &T,
    auth_id: &AuthAccountId,
) -> error_stack::Result<Vec<Account>, KernelError>
where
    T: DependOnAccountQuery + DependOnDatabaseConnection,
{
    let mut connection = deps.database_connection().connection().await?;
    Ok(deps
        .account_query()
        .find_by_auth_id(&mut connection, auth_id)
        .await?
        .into_iter()
        .filter(|account| {
            account.kind() == &AccountKind::Personal && account.deleted_at().is_none()
        })
        .collect())
}

#[allow(dead_code)]
async fn resolve_personal_actor_account<T>(
    deps: &T,
    auth_id: &AuthAccountId,
) -> error_stack::Result<Account, KernelError>
where
    T: DependOnAccountQuery + DependOnDatabaseConnection,
{
    personal_accounts_of(deps, auth_id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| {
            Report::new(KernelError::NotFound)
                .attach_printable("No personal account belongs to the authenticated user")
        })
}

async fn resolve_personal_actor_account_among<T>(
    deps: &T,
    auth_id: &AuthAccountId,
    account_id: &AccountId,
) -> error_stack::Result<Account, KernelError>
where
    T: DependOnAccountQuery + DependOnDatabaseConnection,
{
    personal_accounts_of(deps, auth_id)
        .await?
        .into_iter()
        .find(|account| account.id() == account_id)
        .ok_or_else(|| {
            Report::new(KernelError::PermissionDenied)
                .attach_printable("Authenticated user does not control the required account")
        })
}

async fn find_active_membership<T>(
    deps: &T,
    org_account_id: &AccountId,
    member_account_id: &AccountId,
) -> error_stack::Result<kernel::prelude::entity::OrganizationMembership, KernelError>
where
    T: DependOnOrganizationMembershipRepository + DependOnDatabaseConnection,
{
    let mut connection = deps.database_connection().connection().await?;
    deps.organization_membership_repository()
        .find(&mut connection, org_account_id, member_account_id)
        .await?
        .filter(|membership| membership.status() == &OrganizationMembershipStatus::Active)
        .ok_or_else(|| {
            Report::new(KernelError::PermissionDenied)
                .attach_printable("Actor is not an active member of the organization")
        })
}

async fn resolve_actor_with_active_membership<T>(
    deps: &T,
    auth_id: &AuthAccountId,
    org_account_id: &AccountId,
    required_roles: &[OrgRole],
) -> error_stack::Result<Account, KernelError>
where
    T: DependOnAccountQuery + DependOnOrganizationMembershipRepository + DependOnDatabaseConnection,
{
    let accounts = personal_accounts_of(deps, auth_id).await?;
    if accounts.is_empty() {
        return Err(Report::new(KernelError::PermissionDenied)
            .attach_printable("No personal account belongs to the authenticated user"));
    }
    for account in accounts {
        if let Ok(membership) = find_active_membership(deps, org_account_id, account.id()).await {
            if required_roles.contains(membership.role()) {
                return Ok(account);
            }
        }
    }
    Err(Report::new(KernelError::PermissionDenied)
        .attach_printable("No qualifying actor found for the organization"))
}

async fn require_active_membership<T>(
    deps: &T,
    org_account_id: &AccountId,
    member_account_id: &AccountId,
) -> error_stack::Result<(), KernelError>
where
    T: DependOnOrganizationMembershipRepository + DependOnDatabaseConnection,
{
    find_active_membership(deps, org_account_id, member_account_id)
        .await
        .map(|_| ())
}

#[allow(dead_code)]
async fn require_active_owner_or_admin_membership<T>(
    deps: &T,
    org_account_id: &AccountId,
    member_account_id: &AccountId,
) -> error_stack::Result<(), KernelError>
where
    T: DependOnOrganizationMembershipRepository + DependOnDatabaseConnection,
{
    let mut connection = deps.database_connection().connection().await?;
    match deps
        .organization_membership_repository()
        .find(&mut connection, org_account_id, member_account_id)
        .await?
    {
        Some(membership)
            if membership.status() == &OrganizationMembershipStatus::Active
                && (membership.role() == &OrgRole::Owner
                    || membership.role() == &OrgRole::Admin) =>
        {
            Ok(())
        }
        _ => Err(Report::new(KernelError::PermissionDenied)
            .attach_printable("Actor is not an active owner or admin of the organization")),
    }
}
