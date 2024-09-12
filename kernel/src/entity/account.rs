use destructure::Destructure;
use serde::Deserialize;
use serde::Serialize;
use vodca::{Nameln, Newln, References};

use crate::entity::{CommandEnvelope, DeletedAt, ExpectedEventVersion};

use super::common::CreatedAt;

pub use self::id::*;
pub use self::is_bot::*;
pub use self::name::*;
pub use self::private_key::*;
pub use self::public_key::*;

mod id;
mod is_bot;
mod name;
mod private_key;
mod public_key;

#[derive(Debug, Clone, Hash, References, Newln, Serialize, Deserialize, Destructure)]
pub struct Account {
    id: AccountId,
    name: AccountName,
    private_key: AccountPrivateKey,
    public_key: AccountPublicKey,
    is_bot: AccountIsBot,
    created_at: CreatedAt<Account>,
    deleted_at: Option<DeletedAt<Account>>,
}

#[derive(Debug, Clone, Nameln, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "snake_case")]
pub enum AccountEvent {
    Created {
        name: AccountName,
        private_key: AccountPrivateKey,
        public_key: AccountPublicKey,
        is_bot: AccountIsBot,
    },
    Updated {
        is_bot: AccountIsBot,
    },
    Deleted,
}

impl Account {
    pub fn create(
        name: AccountName,
        private_key: AccountPrivateKey,
        public_key: AccountPublicKey,
        is_bot: AccountIsBot,
    ) -> CommandEnvelope<AccountEvent, Account> {
        let event = AccountEvent::Created {
            name,
            private_key,
            public_key,
            is_bot,
        };
        CommandEnvelope::new(event, Some(ExpectedEventVersion::Nothing))
    }

    pub fn update(is_bot: AccountIsBot) -> CommandEnvelope<AccountEvent, Account> {
        let event = AccountEvent::Updated { is_bot };
        CommandEnvelope::new(event, None)
    }

    pub fn delete() -> CommandEnvelope<AccountEvent, Account> {
        let event = AccountEvent::Deleted;
        CommandEnvelope::new(event, None)
    }
}
