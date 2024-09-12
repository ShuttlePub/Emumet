mod content;
mod id;
mod label;

pub use self::content::*;
pub use self::id::*;
pub use self::label::*;
use destructure::Destructure;
use serde::{Deserialize, Serialize};
use vodca::{Nameln, Newln, References};

use super::{AccountId, CommandEnvelope, CreatedAt, ExpectedEventVersion};

#[derive(Debug, Clone, References, Newln, Destructure, Serialize, Deserialize)]
pub struct Metadata {
    id: MetadataId,
    account_id: AccountId,
    label: MetadataLabel,
    content: MetadataContent,
    created_at: CreatedAt<Metadata>,
}

#[derive(Debug, Clone, Nameln, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MetadataEvent {
    Created {
        account_id: AccountId,
        label: MetadataLabel,
        content: MetadataContent,
    },
    Updated {
        label: MetadataLabel,
        content: MetadataContent,
    },
    Deleted,
}

impl Metadata {
    pub fn create(
        account_id: AccountId,
        label: MetadataLabel,
        content: MetadataContent,
    ) -> CommandEnvelope<MetadataEvent, Metadata> {
        let event = MetadataEvent::Created {
            account_id,
            label,
            content,
        };
        CommandEnvelope::new(event, Some(ExpectedEventVersion::Nothing))
    }

    pub fn update(
        label: MetadataLabel,
        content: MetadataContent,
    ) -> CommandEnvelope<MetadataEvent, Metadata> {
        let event = MetadataEvent::Updated { label, content };
        CommandEnvelope::new(event, None)
    }

    pub fn delete() -> CommandEnvelope<MetadataEvent, Metadata> {
        let event = MetadataEvent::Deleted;
        CommandEnvelope::new(event, None)
    }
}
