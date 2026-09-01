use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::read_model::{
    AccountQuery, DependOnAccountQuery, DependOnOrganizationMembershipQuery,
    OrganizationMembershipQuery,
};
use kernel::interfaces::repository::{
    DependOnOrganizationMembershipRepository, OrganizationMembershipRepository,
};
use kernel::prelude::entity::{
    Account, AccountKind, AuthAccountId, CreatedAt, Nanoid, OrgRole, OrganizationMembership,
    OrganizationMembershipStatus,
};
use kernel::KernelError;
use std::future::Future;

fn permission_denied() -> Report<KernelError> {
    Report::new(KernelError::PermissionDenied)
}

fn not_found() -> Report<KernelError> {
    Report::new(KernelError::NotFound)
}

fn rejected(message: &'static str) -> Report<KernelError> {
    Report::new(KernelError::Rejected).attach_printable(message)
}

pub trait InviteMemberUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnOrganizationMembershipQuery
    + DependOnOrganizationMembershipRepository
    + DependOnTransactionManager
{
    fn invite_member(
        &self,
        auth_id: AuthAccountId,
        org_nanoid: String,
        target_nanoid: String,
        role: OrgRole,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + '_ {
        async move {
            if role == OrgRole::Owner {
                return Err(rejected("Owner invitations are not allowed"));
            }
            let mut conn = self.database_connection().connection().await?;
            let actor_accounts = self
                .account_query()
                .find_by_auth_id(&mut conn, &auth_id)
                .await?;
            let org = self
                .account_query()
                .find_by_nanoid(&mut conn, &Nanoid::<Account>::new(org_nanoid))
                .await?
                .filter(|a| a.kind() == &AccountKind::Organization)
                .ok_or_else(not_found)?;
            let target = self
                .account_query()
                .find_by_nanoid(&mut conn, &Nanoid::<Account>::new(target_nanoid))
                .await?
                .filter(|a| a.kind() == &AccountKind::Personal && a.deleted_at().is_none())
                .ok_or_else(not_found)?;
            let mut actor_membership = None;
            let mut actor = None;
            for account in actor_accounts
                .into_iter()
                .filter(|a| a.kind() == &AccountKind::Personal && a.deleted_at().is_none())
            {
                if let Some(membership) = self
                    .organization_membership_query()
                    .find(&mut conn, org.id(), account.id())
                    .await?
                    .filter(|m| m.status() == &OrganizationMembershipStatus::Active)
                {
                    actor = Some(account);
                    actor_membership = Some(membership);
                    break;
                }
            }
            let actor = actor.ok_or_else(permission_denied)?;
            let actor_membership = actor_membership.ok_or_else(permission_denied)?;
            if !matches!(actor_membership.role(), OrgRole::Owner | OrgRole::Admin) {
                return Err(permission_denied());
            }
            if self
                .organization_membership_query()
                .find(&mut conn, org.id(), target.id())
                .await?
                .is_some()
            {
                return Err(rejected("Account is already an organization member"));
            }
            let membership = OrganizationMembership::new(
                org.id().clone(),
                target.id().clone(),
                role,
                OrganizationMembershipStatus::Pending,
                actor.id().clone(),
                CreatedAt::now(),
            );
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        deps.organization_membership_repository()
                            .create(executor, &membership)
                            .await
                    })
                })
                .await
        }
    }
}

impl<T> InviteMemberUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountQuery
        + DependOnOrganizationMembershipQuery
        + DependOnOrganizationMembershipRepository
        + DependOnTransactionManager
{
}

pub trait AcceptInviteUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnOrganizationMembershipQuery
    + DependOnOrganizationMembershipRepository
    + DependOnTransactionManager
{
    fn accept_invite(
        &self,
        auth_id: AuthAccountId,
        org_nanoid: String,
        actor_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + '_ {
        async move {
            let mut conn = self.database_connection().connection().await?;
            let actor = self
                .account_query()
                .find_by_auth_id(&mut conn, &auth_id)
                .await?
                .into_iter()
                .find(|a| {
                    a.kind() == &AccountKind::Personal
                        && a.deleted_at().is_none()
                        && a.nanoid().as_ref() == &actor_nanoid
                })
                .ok_or_else(permission_denied)?;
            let org = self
                .account_query()
                .find_by_nanoid(&mut conn, &Nanoid::<Account>::new(org_nanoid))
                .await?
                .filter(|a| a.kind() == &AccountKind::Organization)
                .ok_or_else(not_found)?;
            let membership = self
                .organization_membership_query()
                .find(&mut conn, org.id(), actor.id())
                .await?
                .ok_or_else(not_found)?;
            if membership.status() == &OrganizationMembershipStatus::Active {
                return Err(rejected("Invitation is already active"));
            }
            let org_id = org.id().clone();
            let actor_id = actor.id().clone();
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        deps.organization_membership_repository()
                            .update_status(
                                executor,
                                &org_id,
                                &actor_id,
                                OrganizationMembershipStatus::Active,
                            )
                            .await
                    })
                })
                .await
        }
    }
}

impl<T> AcceptInviteUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountQuery
        + DependOnOrganizationMembershipQuery
        + DependOnOrganizationMembershipRepository
        + DependOnTransactionManager
{
}

pub trait ChangeRoleUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnOrganizationMembershipQuery
    + DependOnOrganizationMembershipRepository
    + DependOnTransactionManager
{
    fn change_role(
        &self,
        auth_id: AuthAccountId,
        org_nanoid: String,
        member_nanoid: String,
        new_role: OrgRole,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + '_ {
        async move {
            let mut conn = self.database_connection().connection().await?;
            let actor_accounts = self
                .account_query()
                .find_by_auth_id(&mut conn, &auth_id)
                .await?;
            let org = self
                .account_query()
                .find_by_nanoid(&mut conn, &Nanoid::<Account>::new(org_nanoid))
                .await?
                .filter(|a| a.kind() == &AccountKind::Organization)
                .ok_or_else(not_found)?;
            let member = self
                .account_query()
                .find_by_nanoid(&mut conn, &Nanoid::<Account>::new(member_nanoid))
                .await?
                .filter(|a| a.kind() == &AccountKind::Personal && a.deleted_at().is_none())
                .ok_or_else(not_found)?;
            let mut actor_is_owner = false;
            for account in actor_accounts
                .into_iter()
                .filter(|a| a.kind() == &AccountKind::Personal && a.deleted_at().is_none())
            {
                if self
                    .organization_membership_query()
                    .find(&mut conn, org.id(), account.id())
                    .await?
                    .is_some_and(|m| {
                        m.status() == &OrganizationMembershipStatus::Active
                            && m.role() == &OrgRole::Owner
                    })
                {
                    actor_is_owner = true;
                    break;
                }
            }
            if !actor_is_owner {
                return Err(permission_denied());
            }
            let target = self
                .organization_membership_query()
                .find(&mut conn, org.id(), member.id())
                .await?
                .filter(|m| m.status() == &OrganizationMembershipStatus::Active)
                .ok_or_else(not_found)?;
            let demotes_owner = target.role() == &OrgRole::Owner && new_role != OrgRole::Owner;
            let org_id = org.id().clone();
            let member_id = member.id().clone();
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        if demotes_owner {
                            deps.organization_membership_repository()
                                .lock_active_owner_rows(executor, &org_id)
                                .await?;
                            if deps
                                .organization_membership_repository()
                                .count_active_owners(executor, &org_id)
                                .await?
                                == 1
                            {
                                return Err(rejected("The last active owner cannot be demoted"));
                            }
                        }
                        deps.organization_membership_repository()
                            .update_role(executor, &org_id, &member_id, new_role)
                            .await
                    })
                })
                .await
        }
    }
}

impl<T> ChangeRoleUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountQuery
        + DependOnOrganizationMembershipQuery
        + DependOnOrganizationMembershipRepository
        + DependOnTransactionManager
{
}

pub trait RemoveMemberUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnOrganizationMembershipQuery
    + DependOnOrganizationMembershipRepository
    + DependOnTransactionManager
{
    fn remove_member(
        &self,
        auth_id: AuthAccountId,
        org_nanoid: String,
        member_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + '_ {
        async move {
            let mut conn = self.database_connection().connection().await?;
            let actor_accounts = self
                .account_query()
                .find_by_auth_id(&mut conn, &auth_id)
                .await?;
            let org = self
                .account_query()
                .find_by_nanoid(&mut conn, &Nanoid::<Account>::new(org_nanoid))
                .await?
                .filter(|a| a.kind() == &AccountKind::Organization)
                .ok_or_else(not_found)?;
            let member = self
                .account_query()
                .find_by_nanoid(&mut conn, &Nanoid::<Account>::new(member_nanoid))
                .await?
                .filter(|a| a.kind() == &AccountKind::Personal)
                .ok_or_else(not_found)?;
            let mut actor_can_manage = false;
            for account in actor_accounts
                .into_iter()
                .filter(|a| a.kind() == &AccountKind::Personal && a.deleted_at().is_none())
            {
                if self
                    .organization_membership_query()
                    .find(&mut conn, org.id(), account.id())
                    .await?
                    .is_some_and(|m| {
                        m.status() == &OrganizationMembershipStatus::Active
                            && matches!(m.role(), OrgRole::Owner | OrgRole::Admin)
                    })
                {
                    actor_can_manage = true;
                    break;
                }
            }
            if !actor_can_manage {
                return Err(permission_denied());
            }
            let target = self
                .organization_membership_query()
                .find(&mut conn, org.id(), member.id())
                .await?
                .filter(|m| m.status() == &OrganizationMembershipStatus::Active)
                .ok_or_else(not_found)?;
            if target.role() == &OrgRole::Owner {
                return Err(rejected("Owners cannot be removed"));
            }
            let org_id = org.id().clone();
            let member_id = member.id().clone();
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        deps.organization_membership_repository()
                            .delete(executor, &org_id, &member_id)
                            .await
                    })
                })
                .await
        }
    }
}

impl<T> RemoveMemberUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountQuery
        + DependOnOrganizationMembershipQuery
        + DependOnOrganizationMembershipRepository
        + DependOnTransactionManager
{
}

pub trait LeaveOrganizationUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnOrganizationMembershipQuery
    + DependOnOrganizationMembershipRepository
    + DependOnTransactionManager
{
    fn leave_organization(
        &self,
        auth_id: AuthAccountId,
        org_nanoid: String,
        actor_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + '_ {
        async move {
            let mut conn = self.database_connection().connection().await?;
            let actor = self
                .account_query()
                .find_by_auth_id(&mut conn, &auth_id)
                .await?
                .into_iter()
                .find(|a| {
                    a.kind() == &AccountKind::Personal
                        && a.deleted_at().is_none()
                        && a.nanoid().as_ref() == &actor_nanoid
                })
                .ok_or_else(permission_denied)?;
            let org = self
                .account_query()
                .find_by_nanoid(&mut conn, &Nanoid::<Account>::new(org_nanoid))
                .await?
                .filter(|a| a.kind() == &AccountKind::Organization)
                .ok_or_else(not_found)?;
            let membership = self
                .organization_membership_query()
                .find(&mut conn, org.id(), actor.id())
                .await?
                .filter(|m| m.status() == &OrganizationMembershipStatus::Active)
                .ok_or_else(not_found)?;
            let leaves_as_owner = membership.role() == &OrgRole::Owner;
            let org_id = org.id().clone();
            let actor_id = actor.id().clone();
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        if leaves_as_owner {
                            deps.organization_membership_repository()
                                .lock_active_owner_rows(executor, &org_id)
                                .await?;
                            if deps
                                .organization_membership_repository()
                                .count_active_owners(executor, &org_id)
                                .await?
                                == 1
                            {
                                return Err(rejected("The last active owner cannot leave"));
                            }
                        }
                        deps.organization_membership_repository()
                            .delete(executor, &org_id, &actor_id)
                            .await
                    })
                })
                .await
        }
    }
}

impl<T> LeaveOrganizationUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountQuery
        + DependOnOrganizationMembershipQuery
        + DependOnOrganizationMembershipRepository
        + DependOnTransactionManager
{
}
