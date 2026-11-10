//! One-time Profile username assignment (P1, ADR profile-actor-identity).
//!
//! The owner of a Profile without a username may set it exactly once. There is deliberately no
//! way to change it afterwards: this use case is the only username write path besides setting it
//! at creation, and it refuses Profiles that already carry a username.

use crate::dto::profile::ProfileDto;
use crate::permission::{account_edit, check_organization_account_edit, check_permission};
use crate::service::session_context::OrganizationContext;
use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::read_model::{
    AccountQuery, DependOnAccountQuery, DependOnProfileQuery, DependOnProfileReadModel,
    ProfileQuery, ProfileReadModel,
};
use kernel::interfaces::repository::{
    AggregateRepository, DependOnImageRepository, DependOnProfileRepository, ImageRepository,
};
use kernel::prelude::entity::{AuthAccountId, ImageId, Nanoid, Profile, ProfileUsername};
use kernel::KernelError;
use std::future::Future;

pub trait SetProfileUsernameUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnProfileQuery
    + DependOnProfileReadModel
    + DependOnProfileRepository
    + DependOnImageRepository
    + DependOnPermissionChecker
    + DependOnTransactionManager
{
    /// Errors: `Validation` (format / reserved word, 400), `NotFound` (404), `PermissionDenied`
    /// (not the current owner, 403), `Rejected` (owning Account not active, 422) and
    /// `Concurrency` (the Profile already has a username, the username is taken, or the Profile
    /// changed owner while the request was in flight, 409).
    ///
    /// The owner is authorized first (this may involve a remote permission check) and then
    /// re-verified under a row lock at the very start of the write transaction. A Profile
    /// transfer updates the same row, so "current owner check -> write" is atomic against it: a
    /// request authorized for the previous owner cannot set the (immutable) username on a Profile
    /// that has meanwhile moved to someone else.
    fn set_profile_username<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: String,
        username: String,
    ) -> impl Future<Output = error_stack::Result<ProfileDto, KernelError>> + Send + 'a {
        async move {
            let username = ProfileUsername::parse(username)?;
            let profile_nanoid = Nanoid::<Profile>::new(profile_nanoid);
            let (profile, account) = {
                let mut connection = self.database_connection().connection().await?;
                let profile = self
                    .profile_query()
                    .find_by_nanoid(&mut connection, &profile_nanoid)
                    .await?
                    .ok_or_else(|| {
                        Report::new(KernelError::NotFound).attach_printable(format!(
                            "Profile not found: {}",
                            profile_nanoid.as_ref()
                        ))
                    })?;
                match org_context {
                    Some(context) => check_organization_account_edit(
                        &context.org_account_id,
                        profile.account_id(),
                    )?,
                    None => {
                        check_permission(self, auth_account_id, &account_edit(profile.account_id()))
                            .await?
                    }
                }
                // Only after authorization, and unfiltered: a suspended or banned owner must
                // reach the state check below (422), not look like a missing Account (404).
                let account = self
                    .account_query()
                    .find_by_id_unfiltered(&mut connection, profile.account_id())
                    .await?
                    .ok_or_else(|| {
                        Report::new(KernelError::NotFound)
                            .attach_printable("Owning account not found")
                    })?;
                (profile, account)
            };
            if !account.status().is_active() {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("Cannot modify a suspended or banned account"));
            }
            if profile.username().is_some() {
                return Err(Report::new(KernelError::Concurrency)
                    .attach_printable("Profile already has a username; it is set only once"));
            }

            let profile_id = profile.id().clone();
            let authorized_owner = profile.account_id().clone();
            let deps = self.clone();
            let write_username = username.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let locked = deps
                            .profile_read_model()
                            .lock_by_id_for_update(executor, &profile_id)
                            .await?
                            .ok_or_else(|| {
                                Report::new(KernelError::NotFound)
                                    .attach_printable("Profile no longer exists")
                            })?;
                        if locked.account_id() != &authorized_owner {
                            return Err(Report::new(KernelError::Concurrency).attach_printable(
                                "Profile ownership changed while the request was in flight",
                            ));
                        }
                        if locked.username().is_some() {
                            return Err(Report::new(KernelError::Concurrency).attach_printable(
                                "Profile already has a username; it is set only once",
                            ));
                        }
                        let event = deps
                            .profile_repository()
                            .save(
                                executor,
                                Profile::set_username(profile_id.clone(), write_username.clone()),
                            )
                            .await?;
                        deps.profile_read_model()
                            .set_username(executor, &profile_id, &write_username, &event.version)
                            .await?;
                        Ok(())
                    })
                })
                .await?;

            let mut connection = self.database_connection().connection().await?;
            let updated = self
                .profile_query()
                .find_by_id(&mut connection, profile.id())
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable("Profile no longer exists")
                })?;
            let image_ids: Vec<ImageId> = updated
                .icon()
                .iter()
                .chain(updated.banner().iter())
                .cloned()
                .collect();
            let images = self
                .image_repository()
                .find_by_ids(&mut connection, &image_ids)
                .await?;
            let image_url = |id: &Option<ImageId>| {
                id.as_ref().and_then(|id| {
                    images
                        .iter()
                        .find(|image| image.id() == id)
                        .map(|image| image.url().as_ref().to_string())
                })
            };
            let icon_url = image_url(updated.icon());
            let banner_url = image_url(updated.banner());
            Ok(ProfileDto::new(
                updated,
                account.nanoid().as_ref().to_string(),
                icon_url,
                banner_url,
            ))
        }
    }
}

impl<T> SetProfileUsernameUseCase for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnAccountQuery
        + DependOnProfileQuery
        + DependOnProfileReadModel
        + DependOnProfileRepository
        + DependOnImageRepository
        + DependOnPermissionChecker
        + DependOnTransactionManager
{
}
