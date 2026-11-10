//! Profile x exact-match domain controls (D8P): owner-only CRUD with revision conflict control,
//! and the matching lookup used by later units (D8G / D8R).
//!
//! Every owner-only operation (reads included) runs in ONE transaction that first takes a shared
//! lock on the Profile row and reads the Profile's current owner from that locking read. A
//! Profile transfer updates the same row, so it is serialized with these operations: it either
//! commits before the owner check (the previous owner is then rejected) or waits until the
//! operation has finished.

use crate::dto::domain_control::DomainControlDto;
use crate::service::session_context::OrganizationContext;
use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnDatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::read_model::{AccountQuery, DependOnAccountQuery};
use kernel::interfaces::repository::{
    DependOnDomainControlRepository, DomainControlDeleteOutcome, DomainControlRepository,
};
use kernel::prelude::entity::{
    AccountKind, AuthAccountId, ControlDomain, DomainControl, DomainControlMode, InvalidDomain,
    ProfileId,
};
use kernel::KernelError;
use std::future::Future;

fn invalid_domain(error: InvalidDomain) -> Report<KernelError> {
    Report::new(KernelError::Rejected).attach_printable(error.to_string())
}

fn revision_conflict(message: &'static str) -> Report<KernelError> {
    Report::new(KernelError::Concurrency).attach_printable(message)
}

/// Lock the Profile and require the caller to be its **current** owner. Must run inside the
/// transaction that performs the rest of the operation (the lock lives until it ends).
///
/// * With an organization context (`X-Organization-Id`, membership already verified by the
///   request middleware) the organization must be the Profile's current owning account.
/// * Without one, one of the caller's live personal accounts must be the owning account.
///
/// Ownership is read from the locked Profile row, so after a Profile transfer only the new owner
/// passes and the previous owner is rejected.
pub(crate) async fn require_profile_owner<T>(
    deps: &T,
    executor: &mut <<T as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    auth_account_id: &AuthAccountId,
    org_context: Option<&OrganizationContext>,
    profile_nanoid: &str,
) -> error_stack::Result<ProfileId, KernelError>
where
    T: DependOnAccountQuery + DependOnDomainControlRepository + DependOnDatabaseConnection,
{
    let (profile_id, owner_account_id) = deps
        .domain_control_repository()
        .lock_profile_owner(executor, profile_nanoid)
        .await?
        .ok_or_else(|| {
            Report::new(KernelError::NotFound)
                .attach_printable(format!("Profile not found: {profile_nanoid}"))
        })?;
    let owned = match org_context {
        Some(context) => context.org_account_id == owner_account_id,
        None => deps
            .account_query()
            .find_by_auth_id(executor, auth_account_id)
            .await?
            .iter()
            .any(|account| {
                account.kind() == &AccountKind::Personal
                    && account.deleted_at().is_none()
                    && account.id() == &owner_account_id
            }),
    };
    if !owned {
        return Err(Report::new(KernelError::PermissionDenied)
            .attach_printable("Caller is not the current owner of the Profile"));
    }
    Ok(profile_id)
}

pub trait DomainControlUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnDomainControlRepository
    + DependOnTransactionManager
{
    fn list_domain_controls<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: &'a str,
    ) -> impl Future<Output = error_stack::Result<Vec<DomainControlDto>, KernelError>> + Send + 'a
    {
        async move {
            let deps = self.clone();
            let auth_account_id = auth_account_id.clone();
            let org_context = org_context.cloned();
            let profile_nanoid = profile_nanoid.to_string();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let profile_id = require_profile_owner(
                            &deps,
                            executor,
                            &auth_account_id,
                            org_context.as_ref(),
                            &profile_nanoid,
                        )
                        .await?;
                        let controls = deps
                            .domain_control_repository()
                            .find_by_profile(executor, &profile_id)
                            .await?;
                        Ok(controls
                            .iter()
                            .map(|control| DomainControlDto::new(&profile_nanoid, control))
                            .collect())
                    })
                })
                .await
        }
    }

    fn get_domain_control<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: &'a str,
        domain: &'a str,
    ) -> impl Future<Output = error_stack::Result<DomainControlDto, KernelError>> + Send + 'a {
        async move {
            let deps = self.clone();
            let auth_account_id = auth_account_id.clone();
            let org_context = org_context.cloned();
            let profile_nanoid = profile_nanoid.to_string();
            let domain = domain.to_string();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let profile_id = require_profile_owner(
                            &deps,
                            executor,
                            &auth_account_id,
                            org_context.as_ref(),
                            &profile_nanoid,
                        )
                        .await?;
                        let domain = ControlDomain::parse(&domain).map_err(invalid_domain)?;
                        let control = deps
                            .domain_control_repository()
                            .find(executor, &profile_id, &domain)
                            .await?
                            .ok_or_else(|| {
                                Report::new(KernelError::NotFound)
                                    .attach_printable(format!("Domain control not found: {domain}"))
                            })?;
                        Ok(DomainControlDto::new(&profile_nanoid, &control))
                    })
                })
                .await
        }
    }

    /// Create (`expected_revision` = `None`) or replace (`Some`) the control of one domain.
    /// Create on an existing control and replace on a missing / different revision are conflicts.
    fn put_domain_control<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: &'a str,
        domain: &'a str,
        mode: DomainControlMode,
        expected_revision: Option<u64>,
    ) -> impl Future<Output = error_stack::Result<DomainControlDto, KernelError>> + Send + 'a {
        async move {
            let deps = self.clone();
            let auth_account_id = auth_account_id.clone();
            let org_context = org_context.cloned();
            let profile_nanoid = profile_nanoid.to_string();
            let domain = domain.to_string();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let profile_id = require_profile_owner(
                            &deps,
                            executor,
                            &auth_account_id,
                            org_context.as_ref(),
                            &profile_nanoid,
                        )
                        .await?;
                        let domain = ControlDomain::parse(&domain).map_err(invalid_domain)?;
                        let control = match expected_revision {
                            None => deps
                                .domain_control_repository()
                                .create(executor, &profile_id, &domain, mode)
                                .await?
                                .ok_or_else(|| {
                                    revision_conflict(
                                        "A domain control already exists; send expected_revision",
                                    )
                                })?,
                            Some(expected_revision) => deps
                                .domain_control_repository()
                                .replace(executor, &profile_id, &domain, mode, expected_revision)
                                .await?
                                .ok_or_else(|| {
                                    revision_conflict(
                                        "expected_revision does not match the current control",
                                    )
                                })?,
                        };
                        Ok(DomainControlDto::new(&profile_nanoid, &control))
                    })
                })
                .await
        }
    }

    fn delete_domain_control<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: &'a str,
        domain: &'a str,
        expected_revision: u64,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            let deps = self.clone();
            let auth_account_id = auth_account_id.clone();
            let org_context = org_context.cloned();
            let profile_nanoid = profile_nanoid.to_string();
            let domain = domain.to_string();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let profile_id = require_profile_owner(
                            &deps,
                            executor,
                            &auth_account_id,
                            org_context.as_ref(),
                            &profile_nanoid,
                        )
                        .await?;
                        let domain = ControlDomain::parse(&domain).map_err(invalid_domain)?;
                        match deps
                            .domain_control_repository()
                            .delete(executor, &profile_id, &domain, expected_revision)
                            .await?
                        {
                            DomainControlDeleteOutcome::Deleted => Ok(()),
                            DomainControlDeleteOutcome::NotFound => Err(Report::new(
                                KernelError::NotFound,
                            )
                            .attach_printable(format!("Domain control not found: {domain}"))),
                            DomainControlDeleteOutcome::RevisionMismatch => Err(revision_conflict(
                                "expected_revision does not match the current control",
                            )),
                        }
                    })
                })
                .await
        }
    }
}

impl<T> DomainControlUseCase for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnAccountQuery
        + DependOnDomainControlRepository
        + DependOnTransactionManager
{
}

/// Matching lookup for later units (D8G Follow rejection, D8R relation detachment, display filters).
///
/// Returns the control of `profile_id` that applies to the actor with the given **verified
/// canonical actor ID**, matched on the exact host of that ID. Pass the verified actor ID, never an
/// inbox / sharedInbox / CDN / media URL. An actor ID without a DNS host (IP literal, non-http
/// scheme, ...) never matches. There is no authorization here: this is an internal decision helper.
pub trait FindDomainControlForActorUseCase:
    'static + Sync + Send + DependOnDomainControlRepository + DependOnDatabaseConnection
{
    fn find_domain_control_for_actor<'a>(
        &'a self,
        profile_id: &'a ProfileId,
        verified_actor_id: &'a str,
    ) -> impl Future<Output = error_stack::Result<Option<DomainControl>, KernelError>> + Send + 'a
    {
        async move {
            let Ok(domain) = ControlDomain::from_actor_id(verified_actor_id) else {
                return Ok(None);
            };
            let mut connection = self.database_connection().connection().await?;
            self.domain_control_repository()
                .find(&mut connection, profile_id, &domain)
                .await
        }
    }
}

impl<T> FindDomainControlForActorUseCase for T where
    T: 'static + Sync + Send + DependOnDomainControlRepository + DependOnDatabaseConnection
{
}
