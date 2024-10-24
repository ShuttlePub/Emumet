mod display_name;
mod id;
mod summary;

pub use self::display_name::*;
pub use self::id::*;
pub use self::summary::*;

use super::{
    AccountId, CommandEnvelope, EventEnvelope, EventId, EventVersion, KnownEventVersion, Nanoid,
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

#[derive(Debug, Clone, Nameln, Serialize, Deserialize)]
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
        display_name: Option<ProfileDisplayName>,
        summary: Option<ProfileSummary>,
        icon: Option<ImageId>,
        banner: Option<ImageId>,
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

    pub fn update(
        id: ProfileId,
        display_name: Option<ProfileDisplayName>,
        summary: Option<ProfileSummary>,
        icon: Option<ImageId>,
        banner: Option<ImageId>,
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
                    if let Some(display_name) = display_name {
                        profile.display_name = Some(display_name);
                    }
                    if let Some(summary) = summary {
                        profile.summary = Some(summary);
                    }
                    if let Some(icon) = icon {
                        profile.icon = Some(icon);
                    }
                    if let Some(banner) = banner {
                        profile.banner = Some(banner);
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
        AccountId, EventEnvelope, EventVersion, ImageId, Nanoid, Profile, ProfileDisplayName,
        ProfileId, ProfileSummary,
    };
    use crate::event::EventApplier;
    use uuid::Uuid;

    #[test]
    fn create_profile() {
        let account_id = AccountId::new(Uuid::now_v7());
        let id = ProfileId::new(Uuid::now_v7());
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
            EventVersion::new(Uuid::now_v7()),
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
        let account_id = AccountId::new(Uuid::now_v7());
        let id = ProfileId::new(Uuid::now_v7());
        let nano_id = Nanoid::default();
        let profile = Profile::new(
            id.clone(),
            account_id.clone(),
            None,
            None,
            None,
            None,
            EventVersion::new(Uuid::now_v7()),
            nano_id.clone(),
        );
        let display_name = ProfileDisplayName::new("display_name".to_string());
        let summary = ProfileSummary::new("summary".to_string());
        let icon = ImageId::new(Uuid::now_v7());
        let banner = ImageId::new(Uuid::now_v7());
        let update_event = Profile::update(
            id.clone(),
            Some(display_name.clone()),
            Some(summary.clone()),
            Some(icon.clone()),
            Some(banner.clone()),
        );
        let version = EventVersion::new(Uuid::now_v7());
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
}
