use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{DependOnProfileEventStore, ProfileEventStore};
use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
use kernel::interfaces::repository::{DependOnImageRepository, ImageRepository};
use kernel::prelude::entity::{EventId, FieldAction, ImageId, ImageUrl, Profile, ProfileId};
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
            let mut transaction = self.database_connection().get_executor().await?;
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

async fn resolve_image_id<T: DependOnImageRepository + ?Sized>(
    deps: &T,
    executor: &mut <<T as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    url: Option<&str>,
) -> error_stack::Result<Option<ImageId>, KernelError> {
    let Some(url) = url else {
        return Ok(None);
    };
    let image_url = ImageUrl::new(url.to_string());
    image_url.validate()?;
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

pub(crate) async fn resolve_field_action_image_id<T: DependOnImageRepository + ?Sized>(
    deps: &T,
    executor: &mut <<T as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    action: &FieldAction<String>,
) -> error_stack::Result<FieldAction<ImageId>, KernelError> {
    match action {
        FieldAction::Unchanged => Ok(FieldAction::Unchanged),
        FieldAction::Clear => Ok(FieldAction::Clear),
        FieldAction::Set(url) => {
            match resolve_image_id(deps, executor, Some(url.as_str())).await? {
                Some(id) => Ok(FieldAction::Set(id)),
                None => Err(Report::new(KernelError::Internal)
                    .attach_printable("Image resolution returned no ID for a provided URL")),
            }
        }
    }
}
