mod display_name;
mod summary;

pub use self::display_name::*;
pub use self::summary::*;
use crate::entity::image::ImageId;
use destructure::Destructure;
use serde::{Deserialize, Serialize};
use vodca::{Newln, References};

use super::{AccountId, CommandEnvelope, ExpectedEventVersion};

#[derive(Debug, Clone, Hash, References, Newln, Destructure, Serialize, Deserialize)]
pub struct Profile {
    id: AccountId,
    display_name: Option<ProfileDisplayName>,
    summary: Option<ProfileSummary>,
    icon: Option<ImageId>,
    banner: Option<ImageId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProfileEvent {
    Created {
        display_name: Option<ProfileDisplayName>,
        summary: Option<ProfileSummary>,
        icon: Option<ImageId>,
        banner: Option<ImageId>,
    },
    Updated {
        display_name: Option<ProfileDisplayName>,
        summary: Option<ProfileSummary>,
        icon: Option<ImageId>,
        banner: Option<ImageId>,
    },
    Deleted,
}

impl Profile {
    pub fn create(
        display_name: Option<ProfileDisplayName>,
        summary: Option<ProfileSummary>,
        icon: Option<ImageId>,
        banner: Option<ImageId>,
    ) -> CommandEnvelope<ProfileEvent, Profile> {
        let event = ProfileEvent::Created {
            display_name,
            summary,
            icon,
            banner,
        };
        CommandEnvelope::new(event, Some(ExpectedEventVersion::Nothing))
    }

    pub fn update(
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
        CommandEnvelope::new(event, None)
    }

    pub fn delete() -> CommandEnvelope<ProfileEvent, Profile> {
        let event = ProfileEvent::Deleted;
        CommandEnvelope::new(event, None)
    }
}
