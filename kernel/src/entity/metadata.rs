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
        id: impl Into<i64>,
        account_id: impl Into<i64>,
        label: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: Id::new(id),
            account_id: Id::new(account_id),
            label: Label::new(label),
            content: Content::new(content),
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
