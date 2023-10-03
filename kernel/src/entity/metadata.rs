mod content;
mod label;

pub use self::content::*;
pub use self::label::*;

use super::Account;
use super::Id;

pub struct Metadata {
    id: Id<Metadata>,
    account_id: Id<Account>,
    label: Label,
    content: Content,
}

impl Metadata {
    pub fn new(
        id: impl Into<Id<Metadata>>,
        account_id: impl Into<Id<Account>>,
        label: impl Into<Label>,
        content: impl Into<Content>,
    ) -> Self {
        Self {
            id: id.into(),
            account_id: account_id.into(),
            label: label.into(),
            content: content.into(),
        }
    }

    pub fn id(&self) -> &Id<Metadata> {
        &self.id
    }

    pub fn account_id(&self) -> &Id<Account> {
        &self.account_id
    }

    pub fn label(&self) -> &Label {
        &self.label
    }

    pub fn content(&self) -> &Content {
        &self.content
    }
}
