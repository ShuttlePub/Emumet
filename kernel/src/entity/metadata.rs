mod content;
mod id;
mod label;

pub use self::content::*;
pub use self::id::*;
pub use self::label::*;

use super::AccountId;

pub struct Metadata {
    id: MetadataId,
    account_id: AccountId,
    label: MetadataLabel,
    content: MetadataContent,
}

impl Metadata {
    pub fn new(
        id: MetadataId,
        account_id: AccountId,
        label: MetadataLabel,
        content: MetadataContent,
    ) -> Self {
        Self {
            id,
            account_id,
            label,
            content,
        }
    }

    pub fn id(&self) -> &MetadataId {
        &self.id
    }

    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    pub fn label(&self) -> &MetadataLabel {
        &self.label
    }

    pub fn content(&self) -> &MetadataContent {
        &self.content
    }
}
