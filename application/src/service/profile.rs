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
use kernel::interfaces::repository::{DependOnImageRepository, ImageRepository};
use kernel::prelude::entity::{
    Account, AuthAccountId, EventId, FieldAction, ImageId, ImageUrl, Nanoid, Profile,
    ProfileDisplayName, ProfileId, ProfileSummary,
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
    + DependOnImageRepository
    + DependOnPermissionChecker
{
    fn get_profiles_batch(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoids: Vec<String>,
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileDto>, KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoids: Vec<Nanoid<Account>> = account_nanoids
                .into_iter()
                .map(Nanoid::<Account>::new)
                .collect();
            let accounts = self
                .account_query_processor()
                .find_by_nanoids(&mut transaction, &nanoids)
                .await?;

            let mut permitted_accounts = Vec::new();
            for account in accounts {
                if check_permission(self, auth_account_id, &account_view(account.id()))
                    .await
                    .is_ok()
                {
                    permitted_accounts.push(account);
                }
            }

            if permitted_accounts.is_empty() {
                return Ok(Vec::new());
            }

            let account_ids: Vec<_> = permitted_accounts.iter().map(|a| a.id().clone()).collect();
            let nanoid_map: std::collections::HashMap<_, _> = permitted_accounts
                .iter()
                .map(|a| (a.id().clone(), a.nanoid().as_ref().to_string()))
                .collect();

            let profiles = self
                .profile_query_processor()
                .find_by_account_ids(&mut transaction, &account_ids)
                .await?;

            let all_image_ids: Vec<ImageId> = {
                let unique: std::collections::HashSet<_> = profiles
                    .iter()
                    .flat_map(|p| {
                        p.icon()
                            .as_ref()
                            .into_iter()
                            .chain(p.banner().as_ref())
                            .cloned()
                    })
                    .collect();
                unique.into_iter().collect()
            };

            let image_map: std::collections::HashMap<ImageId, String> = if all_image_ids.is_empty()
            {
                std::collections::HashMap::new()
            } else {
                self.image_repository()
                    .find_by_ids(&mut transaction, &all_image_ids)
                    .await?
                    .into_iter()
                    .map(|img| (img.id().clone(), img.url().as_ref().to_string()))
                    .collect()
            };

            let mut dtos = Vec::new();
            for profile in profiles {
                let account_nanoid = match nanoid_map.get(profile.account_id()) {
                    Some(n) => n.clone(),
                    None => continue,
                };
                let icon_url = profile
                    .icon()
                    .as_ref()
                    .and_then(|id| image_map.get(id).cloned());
                let banner_url = profile
                    .banner()
                    .as_ref()
                    .and_then(|id| image_map.get(id).cloned());
                dtos.push(ProfileDto::new(
                    profile,
                    account_nanoid,
                    icon_url,
                    banner_url,
                ));
            }
            Ok(dtos)
        }
    }
}

impl<T> GetProfileUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnProfileQueryProcessor
        + DependOnAccountQueryProcessor
        + DependOnImageRepository
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
    + DependOnImageRepository
    + DependOnPermissionChecker
{
    fn create_profile(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoid: String,
        display_name: Option<String>,
        summary: Option<String>,
        icon_url: Option<String>,
        banner_url: Option<String>,
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

            let icon = resolve_image_id(self, &mut transaction, icon_url.as_deref()).await?;
            let banner = resolve_image_id(self, &mut transaction, banner_url.as_deref()).await?;

            let account_nanoid_str = account.nanoid().as_ref().to_string();
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

            let icon_url =
                resolve_image_url(self, &mut transaction, profile.icon().as_ref()).await?;
            let banner_url =
                resolve_image_url(self, &mut transaction, profile.banner().as_ref()).await?;

            Ok(ProfileDto::new(
                profile,
                account_nanoid_str,
                icon_url,
                banner_url,
            ))
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
        + DependOnImageRepository
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
    + DependOnImageRepository
    + DependOnPermissionChecker
{
    fn edit_profile(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoid: String,
        display_name: Option<String>,
        summary: Option<String>,
        icon_url: FieldAction<String>,
        banner_url: FieldAction<String>,
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

            let icon = resolve_field_action_image_id(self, &mut transaction, &icon_url).await?;
            let banner = resolve_field_action_image_id(self, &mut transaction, &banner_url).await?;

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
        + DependOnImageRepository
        + DependOnPermissionChecker
{
}

async fn resolve_image_url<T: DependOnImageRepository + ?Sized>(
    deps: &T,
    executor: &mut <<T as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    image_id: Option<&ImageId>,
) -> error_stack::Result<Option<String>, KernelError> {
    let Some(id) = image_id else {
        return Ok(None);
    };
    let image = deps.image_repository().find_by_id(executor, id).await?;
    if image.is_none() {
        tracing::warn!(?id, "Image record not found for referenced ImageId");
    }
    Ok(image.map(|img| img.url().as_ref().to_string()))
}

async fn resolve_image_id<T: DependOnImageRepository + ?Sized>(
    deps: &T,
    executor: &mut <<T as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    url: Option<&str>,
) -> error_stack::Result<Option<ImageId>, KernelError> {
    let Some(url) = url else {
        return Ok(None);
    };
    if url.is_empty() {
        return Err(
            Report::new(KernelError::Rejected).attach_printable("Image URL must not be empty")
        );
    }
    let image_url = ImageUrl::new(url.to_string());
    let image = deps
        .image_repository()
        .find_by_url(executor, &image_url)
        .await?
        .ok_or_else(|| {
            Report::new(KernelError::NotFound)
                .attach_printable(format!("Image not found with URL: {}", url))
        })?;
    Ok(Some(image.id().clone()))
}

async fn resolve_field_action_image_id<T: DependOnImageRepository + ?Sized>(
    deps: &T,
    executor: &mut <<T as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    action: &FieldAction<String>,
) -> error_stack::Result<FieldAction<ImageId>, KernelError> {
    match action {
        FieldAction::Unchanged => Ok(FieldAction::Unchanged),
        FieldAction::Clear => Ok(FieldAction::Clear),
        FieldAction::Set(url) => {
            // resolve_image_id with Some(url) always returns Ok(Some(id)) or Err(NotFound)
            let id = resolve_image_id(deps, executor, Some(url.as_str()))
                .await?
                .expect("resolve_image_id with Some input never returns Ok(None)");
            Ok(FieldAction::Set(id))
        }
    }
}
