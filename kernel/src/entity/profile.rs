mod display_name;
mod id;
mod summary;

pub use self::display_name::*;
pub use self::id::*;
pub use self::summary::*;

use super::{
    AccountId, CommandEnvelope, EventEnvelope, EventId, EventVersion, FieldAction,
    KnownEventVersion, Nanoid,
};
use crate::entity::image::ImageId;
use crate::event::EventApplier;
use crate::KernelError;
use destructure::Destructure;
use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{Nameln, Newln, References};

#[derive(
    Debug, Clone, Hash, Eq, PartialEq, References, Newln, Destructure, Serialize, Deserialize,
)]
pub struct Profile {
    id: ProfileId,
    account_id: AccountId,
    display_name: Option<ProfileDisplayName>,
    summary: Option<ProfileSummary>,
    icon: Option<ImageId>,
    banner: Option<ImageId>,
    version: EventVersion<Profile>,
    nanoid: Nanoid<Profile>,
}

#[derive(Debug, Clone, PartialEq, Nameln, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[vodca(prefix = "profile", snake_case)]
pub enum ProfileEvent {
    Created {
        account_id: AccountId,
        display_name: Option<ProfileDisplayName>,
        summary: Option<ProfileSummary>,
        icon: Option<ImageId>,
        banner: Option<ImageId>,
        nanoid: Nanoid<Profile>,
    },
    Updated {
        #[serde(default, skip_serializing_if = "FieldAction::is_unchanged")]
        display_name: FieldAction<ProfileDisplayName>,
        #[serde(default, skip_serializing_if = "FieldAction::is_unchanged")]
        summary: FieldAction<ProfileSummary>,
        #[serde(default, skip_serializing_if = "FieldAction::is_unchanged")]
        icon: FieldAction<ImageId>,
        #[serde(default, skip_serializing_if = "FieldAction::is_unchanged")]
        banner: FieldAction<ImageId>,
    },
}

impl Profile {
    pub fn create(
        id: ProfileId,
        account_id: AccountId,
        display_name: Option<ProfileDisplayName>,
        summary: Option<ProfileSummary>,
        icon: Option<ImageId>,
        banner: Option<ImageId>,
        nano_id: Nanoid<Profile>,
    ) -> CommandEnvelope<ProfileEvent, Profile> {
        let event = ProfileEvent::Created {
            account_id,
            display_name,
            summary,
            icon,
            banner,
            nanoid: nano_id,
        };
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(KnownEventVersion::Nothing),
        )
    }

    /// Last-write-wins: no optimistic concurrency check.
    pub fn update(
        id: ProfileId,
        display_name: FieldAction<ProfileDisplayName>,
        summary: FieldAction<ProfileSummary>,
        icon: FieldAction<ImageId>,
        banner: FieldAction<ImageId>,
    ) -> CommandEnvelope<ProfileEvent, Profile> {
        let event = ProfileEvent::Updated {
            display_name,
            summary,
            icon,
            banner,
        };
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }
}

impl EventApplier for Profile {
    type Event = ProfileEvent;
    const ENTITY_NAME: &'static str = "Profile";

    fn apply(
        entity: &mut Option<Self>,
        event: EventEnvelope<Self::Event, Self>,
    ) -> error_stack::Result<(), KernelError> {
        match event.event {
            ProfileEvent::Created {
                account_id,
                display_name,
                summary,
                icon,
                banner,
                nanoid: nano_id,
            } => {
                if let Some(entity) = entity {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::already_exists(entity)));
                }
                *entity = Some(Profile {
                    id: ProfileId::new(event.id),
                    account_id,
                    display_name,
                    summary,
                    icon,
                    banner,
                    version: event.version,
                    nanoid: nano_id,
                });
            }
            ProfileEvent::Updated {
                display_name,
                summary,
                icon,
                banner,
            } => {
                if let Some(profile) = entity {
                    match display_name {
                        FieldAction::Unchanged => {}
                        FieldAction::Clear => profile.display_name = None,
                        FieldAction::Set(v) => profile.display_name = Some(v),
                    }
                    match summary {
                        FieldAction::Unchanged => {}
                        FieldAction::Clear => profile.summary = None,
                        FieldAction::Set(v) => profile.summary = Some(v),
                    }
                    match icon {
                        FieldAction::Unchanged => {}
                        FieldAction::Clear => profile.icon = None,
                        FieldAction::Set(v) => profile.icon = Some(v),
                    }
                    match banner {
                        FieldAction::Unchanged => {}
                        FieldAction::Clear => profile.banner = None,
                        FieldAction::Set(v) => profile.banner = Some(v),
                    }
                    profile.version = event.version;
                } else {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::entity::{
        AccountId, EventEnvelope, EventVersion, FieldAction, ImageId, Nanoid, Profile,
        ProfileDisplayName, ProfileId, ProfileSummary,
    };
    use crate::event::EventApplier;
    use crate::test_utils::ProfileBuilder;

    #[test]
    fn create_profile() {
        crate::ensure_generator_initialized();
        let account_id = AccountId::default();
        let id = ProfileId::new(crate::generate_id());
        let nano_id = Nanoid::default();
        let create_event = Profile::create(
            id.clone(),
            account_id.clone(),
            None,
            None,
            None,
            None,
            nano_id.clone(),
        );
        let envelope = EventEnvelope::new(
            create_event.id().clone(),
            create_event.event().clone(),
            EventVersion::default(),
        );
        let mut profile = None;
        Profile::apply(&mut profile, envelope).unwrap();
        assert!(profile.is_some());
        let profile = profile.unwrap();
        assert_eq!(profile.id(), &id);
        assert_eq!(profile.account_id(), &account_id);
        assert!(profile.display_name().is_none());
        assert!(profile.summary().is_none());
        assert!(profile.icon().is_none());
        assert!(profile.banner().is_none());
        assert_eq!(profile.nanoid(), &nano_id);
    }

    #[test]
    fn update_profile() {
        crate::ensure_generator_initialized();
        let account_id = AccountId::default();
        let id = ProfileId::new(crate::generate_id());
        let nano_id = Nanoid::default();
        let profile = ProfileBuilder::new()
            .id(id.clone())
            .account_id(account_id.clone())
            .display_name(None::<String>)
            .summary(None::<String>)
            .nanoid(nano_id.clone())
            .build();
        let display_name = ProfileDisplayName::new("display_name".to_string());
        let summary = ProfileSummary::new("summary".to_string());
        let icon = ImageId::new(crate::generate_id());
        let banner = ImageId::new(crate::generate_id());
        let update_event = Profile::update(
            id.clone(),
            FieldAction::Set(display_name.clone()),
            FieldAction::Set(summary.clone()),
            FieldAction::Set(icon.clone()),
            FieldAction::Set(banner.clone()),
        );
        let version = EventVersion::default();
        let envelope = EventEnvelope::new(
            update_event.id().clone(),
            update_event.event().clone(),
            version.clone(),
        );
        let mut profile = Some(profile);
        Profile::apply(&mut profile, envelope).unwrap();
        assert!(profile.is_some());
        let profile = profile.unwrap();
        assert_eq!(profile.id(), &id);
        assert_eq!(profile.account_id(), &account_id);
        assert_eq!(profile.display_name().as_ref().unwrap(), &display_name);
        assert_eq!(profile.summary().as_ref().unwrap(), &summary);
        assert_eq!(profile.icon().as_ref().unwrap(), &icon);
        assert_eq!(profile.banner().as_ref().unwrap(), &banner);
        assert_eq!(profile.version(), &version);
        assert_eq!(profile.nanoid(), &nano_id);
    }

    #[test]
    fn clear_icon_and_banner() {
        crate::ensure_generator_initialized();
        let id = ProfileId::new(crate::generate_id());
        let icon = ImageId::new(crate::generate_id());
        let banner = ImageId::new(crate::generate_id());
        let profile = ProfileBuilder::new()
            .id(id.clone())
            .display_name(None::<String>)
            .summary(None::<String>)
            .icon(Some(icon))
            .banner(Some(banner))
            .build();

        let update_event = Profile::update(
            id,
            FieldAction::Unchanged,
            FieldAction::Unchanged,
            FieldAction::Clear,
            FieldAction::Clear,
        );
        let new_version = EventVersion::default();
        let envelope = EventEnvelope::new(
            update_event.id().clone(),
            update_event.event().clone(),
            new_version,
        );
        let mut profile = Some(profile);
        Profile::apply(&mut profile, envelope).unwrap();
        let profile = profile.unwrap();
        assert!(profile.icon().is_none());
        assert!(profile.banner().is_none());
    }
}
