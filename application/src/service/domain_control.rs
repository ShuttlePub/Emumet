//! Profile x exact-match domain controls (D8P): owner-only CRUD with revision conflict control,
//! and the matching lookup used by later units (D8G / D8R).

use crate::dto::domain_control::DomainControlDto;
use crate::service::session_context::OrganizationContext;
use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::read_model::{
    AccountQuery, DependOnAccountQuery, DependOnProfileQuery, ProfileQuery,
};
use kernel::interfaces::repository::{
    DependOnDomainControlRepository, DomainControlDeleteOutcome, DomainControlRepository,
};
use kernel::prelude::entity::{
    AccountKind, AuthAccountId, ControlDomain, DomainControl, DomainControlMode, Nanoid, Profile,
    ProfileId,
};
use kernel::KernelError;
use std::future::Future;

fn invalid_domain(error: kernel::prelude::entity::InvalidDomain) -> Report<KernelError> {
    Report::new(KernelError::Rejected).attach_printable(error.to_string())
}

fn revision_conflict(message: &'static str) -> Report<KernelError> {
    Report::new(KernelError::Concurrency).attach_printable(message)
}

/// Resolve the Profile and require the caller to be its **current** owner.
///
/// * With an organization context (`X-Organization-Id`, membership already verified by the
///   request middleware) the organization must be the Profile's current owning account.
/// * Without one, one of the caller's live personal accounts must be the owning account.
///
/// Ownership is read from the Profile's current `account_id`, so after a Profile transfer only
/// the new owner passes and the previous owner is rejected.
async fn require_profile_owner<T>(
    deps: &T,
    auth_account_id: &AuthAccountId,
    org_context: Option<&OrganizationContext>,
    profile_nanoid: &str,
) -> error_stack::Result<(ProfileId, String), KernelError>
where
    T: DependOnProfileQuery + DependOnAccountQuery + DependOnDatabaseConnection,
{
    let mut connection = deps.database_connection().connection().await?;
    let profile = deps
        .profile_query()
        .find_by_nanoid(&mut connection, &Nanoid::<Profile>::new(profile_nanoid))
        .await?
        .ok_or_else(|| {
            Report::new(KernelError::NotFound)
                .attach_printable(format!("Profile not found: {profile_nanoid}"))
        })?;
    let owned = match org_context {
        Some(context) => &context.org_account_id == profile.account_id(),
        None => deps
            .account_query()
            .find_by_auth_id(&mut connection, auth_account_id)
            .await?
            .iter()
            .any(|account| {
                account.kind() == &AccountKind::Personal
                    && account.deleted_at().is_none()
                    && account.id() == profile.account_id()
            }),
    };
    if !owned {
        return Err(Report::new(KernelError::PermissionDenied)
            .attach_printable("Caller is not the current owner of the Profile"));
    }
    Ok((profile.id().clone(), profile.nanoid().as_ref().to_string()))
}

pub trait DomainControlUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnProfileQuery
    + DependOnAccountQuery
    + DependOnDomainControlRepository
    + DependOnDatabaseConnection
{
    fn list_domain_controls<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: &'a str,
    ) -> impl Future<Output = error_stack::Result<Vec<DomainControlDto>, KernelError>> + Send + 'a
    {
        async move {
            let (profile_id, profile_nanoid) =
                require_profile_owner(self, auth_account_id, org_context, profile_nanoid).await?;
            let mut connection = self.database_connection().connection().await?;
            let controls = self
                .domain_control_repository()
                .find_by_profile(&mut connection, &profile_id)
                .await?;
            Ok(controls
                .iter()
                .map(|control| DomainControlDto::new(&profile_nanoid, control))
                .collect())
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
            let (profile_id, profile_nanoid) =
                require_profile_owner(self, auth_account_id, org_context, profile_nanoid).await?;
            let domain = ControlDomain::parse(domain).map_err(invalid_domain)?;
            let mut connection = self.database_connection().connection().await?;
            let control = self
                .domain_control_repository()
                .find(&mut connection, &profile_id, &domain)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable(format!("Domain control not found: {domain}"))
                })?;
            Ok(DomainControlDto::new(&profile_nanoid, &control))
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
            let (profile_id, profile_nanoid) =
                require_profile_owner(self, auth_account_id, org_context, profile_nanoid).await?;
            let domain = ControlDomain::parse(domain).map_err(invalid_domain)?;
            let mut connection = self.database_connection().connection().await?;
            let control = match expected_revision {
                None => self
                    .domain_control_repository()
                    .create(&mut connection, &profile_id, &domain, mode)
                    .await?
                    .ok_or_else(|| {
                        revision_conflict("A domain control already exists; send expected_revision")
                    })?,
                Some(expected_revision) => self
                    .domain_control_repository()
                    .replace(
                        &mut connection,
                        &profile_id,
                        &domain,
                        mode,
                        expected_revision,
                    )
                    .await?
                    .ok_or_else(|| {
                        revision_conflict("expected_revision does not match the current control")
                    })?,
            };
            Ok(DomainControlDto::new(&profile_nanoid, &control))
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
            let (profile_id, _) =
                require_profile_owner(self, auth_account_id, org_context, profile_nanoid).await?;
            let domain = ControlDomain::parse(domain).map_err(invalid_domain)?;
            let mut connection = self.database_connection().connection().await?;
            match self
                .domain_control_repository()
                .delete(&mut connection, &profile_id, &domain, expected_revision)
                .await?
            {
                DomainControlDeleteOutcome::Deleted => Ok(()),
                DomainControlDeleteOutcome::NotFound => Err(Report::new(KernelError::NotFound)
                    .attach_printable(format!("Domain control not found: {domain}"))),
                DomainControlDeleteOutcome::RevisionMismatch => Err(revision_conflict(
                    "expected_revision does not match the current control",
                )),
            }
        }
    }
}

impl<T> DomainControlUseCase for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnProfileQuery
        + DependOnAccountQuery
        + DependOnDomainControlRepository
        + DependOnDatabaseConnection
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
