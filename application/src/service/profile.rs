use crate::permission::{account_edit, account_view, check_permission};
use crate::transfer::profile::ProfileDto;
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use adapter::processor::profile::{
    DependOnProfileCommandProcessor, DependOnProfileQueryProcessor, ProfileCommandProcessor,
    ProfileQueryProcessor,
};
use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{DependOnProfileEventStore, ProfileEventStore};
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
use kernel::prelude::entity::{
    Account, AuthAccountId, EventId, ImageId, Nanoid, Profile, ProfileDisplayName, ProfileId,
    ProfileSummary,
};
use kernel::KernelError;
use std::future::Future;

pub trait UpdateProfile:
    'static + DependOnDatabaseConnection + DependOnProfileReadModel + DependOnProfileEventStore
{
    fn update_profile(
        &self,
        profile_id: ProfileId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;
            let existing = self
                .profile_read_model()
                .find_by_id(&mut transaction, &profile_id)
                .await?;
            let event_id = EventId::from(profile_id.clone());

            if let Some(profile) = existing {
                let events = self
                    .profile_event_store()
                    .find_by_id(&mut transaction, &event_id, Some(profile.version()))
                    .await?;
                if events
                    .last()
                    .map(|event| &event.version != profile.version())
                    .unwrap_or(false)
                {
                    let mut profile = Some(profile);
                    for event in events {
                        Profile::apply(&mut profile, event)?;
                    }
                    if let Some(profile) = profile {
                        self.profile_read_model()
                            .update(&mut transaction, &profile)
                            .await?;
                    } else {
                        self.profile_read_model()
                            .delete(&mut transaction, &profile_id)
                            .await?;
                    }
                }
            } else {
                let events = self
                    .profile_event_store()
                    .find_by_id(&mut transaction, &event_id, None)
                    .await?;
                if !events.is_empty() {
                    let mut profile = None;
                    for event in events {
                        Profile::apply(&mut profile, event)?;
                    }
                    if let Some(profile) = profile {
                        self.profile_read_model()
                            .create(&mut transaction, &profile)
                            .await?;
                    }
                }
            }
            Ok(())
        }
    }
}

impl<T> UpdateProfile for T where
    T: 'static + DependOnDatabaseConnection + DependOnProfileReadModel + DependOnProfileEventStore
{
}

pub trait GetProfileUseCase:
    'static
    + Sync
    + Send
    + DependOnProfileQueryProcessor
    + DependOnAccountQueryProcessor
    + DependOnPermissionChecker
{
    fn get_profile(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<ProfileDto, KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoid = kernel::prelude::entity::Nanoid::<Account>::new(account_nanoid);
            let account = self
                .account_query_processor()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &account_view(account.id())).await?;

            let profile = self
                .profile_query_processor()
                .find_by_account_id(&mut transaction, account.id())
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable("Profile not found for this account")
                })?;

            Ok(ProfileDto::from(profile))
        }
    }
}

impl<T> GetProfileUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnProfileQueryProcessor
        + DependOnAccountQueryProcessor
        + DependOnPermissionChecker
{
}

pub trait CreateProfileUseCase:
    'static
    + Sync
    + Send
    + DependOnProfileCommandProcessor
    + DependOnProfileQueryProcessor
    + DependOnAccountQueryProcessor
    + DependOnPermissionChecker
{
    fn create_profile(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoid: String,
        display_name: Option<String>,
        summary: Option<String>,
        icon: Option<ImageId>,
        banner: Option<ImageId>,
    ) -> impl Future<Output = error_stack::Result<ProfileDto, KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoid = kernel::prelude::entity::Nanoid::<Account>::new(account_nanoid);
            let account = self
                .account_query_processor()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &account_edit(account.id())).await?;

            let existing_profile = self
                .profile_query_processor()
                .find_by_account_id(&mut transaction, account.id())
                .await?;
            if existing_profile.is_some() {
                return Err(Report::new(KernelError::Concurrency)
                    .attach_printable("Profile already exists for this account"));
            }

            let account_id = account.id().clone();
            let profile_nanoid = Nanoid::<Profile>::default();
            let profile = self
                .profile_command_processor()
                .create(
                    &mut transaction,
                    account_id,
                    display_name.map(ProfileDisplayName::new),
                    summary.map(ProfileSummary::new),
                    icon,
                    banner,
                    profile_nanoid,
                )
                .await?;

            Ok(ProfileDto::from(profile))
        }
    }
}

impl<T> CreateProfileUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnProfileCommandProcessor
        + DependOnProfileQueryProcessor
        + DependOnAccountQueryProcessor
        + DependOnPermissionChecker
{
}

pub trait EditProfileUseCase:
    'static
    + Sync
    + Send
    + DependOnProfileCommandProcessor
    + DependOnProfileQueryProcessor
    + DependOnAccountQueryProcessor
    + DependOnPermissionChecker
{
    fn edit_profile(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoid: String,
        display_name: Option<String>,
        summary: Option<String>,
        icon: Option<ImageId>,
        banner: Option<ImageId>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoid = kernel::prelude::entity::Nanoid::<Account>::new(account_nanoid);
            let account = self
                .account_query_processor()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &account_edit(account.id())).await?;

            let profile = self
                .profile_query_processor()
                .find_by_account_id(&mut transaction, account.id())
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable("Profile not found for this account")
                })?;

            let profile_id = profile.id().clone();
            let current_version = profile.version().clone();
            self.profile_command_processor()
                .update(
                    &mut transaction,
                    profile_id,
                    display_name.map(ProfileDisplayName::new),
                    summary.map(ProfileSummary::new),
                    icon,
                    banner,
                    current_version,
                )
                .await?;

            Ok(())
        }
    }
}

impl<T> EditProfileUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnProfileCommandProcessor
        + DependOnProfileQueryProcessor
        + DependOnAccountQueryProcessor
        + DependOnPermissionChecker
{
}

pub trait DeleteProfileUseCase:
    'static
    + Sync
    + Send
    + DependOnProfileCommandProcessor
    + DependOnProfileQueryProcessor
    + DependOnAccountQueryProcessor
    + DependOnPermissionChecker
{
    fn delete_profile(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoid = kernel::prelude::entity::Nanoid::<Account>::new(account_nanoid);
            let account = self
                .account_query_processor()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &account_edit(account.id())).await?;

            let profile = self
                .profile_query_processor()
                .find_by_account_id(&mut transaction, account.id())
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable("Profile not found for this account")
                })?;

            let profile_id = profile.id().clone();
            let current_version = profile.version().clone();
            self.profile_command_processor()
                .delete(&mut transaction, profile_id, current_version)
                .await?;

            Ok(())
        }
    }
}

impl<T> DeleteProfileUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnProfileCommandProcessor
        + DependOnProfileQueryProcessor
        + DependOnAccountQueryProcessor
        + DependOnPermissionChecker
{
}
