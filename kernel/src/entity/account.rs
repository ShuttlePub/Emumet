use destructure::Destructure;
use error_stack::{Report, ResultExt};
use serde::Deserialize;
use serde::Serialize;
use vodca::{Nameln, Newln, References};

use super::common::CreatedAt;
use crate::entity::{CommandEnvelope, DeletedAt, EventEnvelope, EventId, KnownEventVersion};
use crate::event::EventApplier;
use crate::KernelError;

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

#[derive(
    Debug, Clone, Hash, Eq, PartialEq, References, Newln, Serialize, Deserialize, Destructure,
)]
pub struct Account {
    id: AccountId,
    name: AccountName,
    private_key: AccountPrivateKey,
    public_key: AccountPublicKey,
    is_bot: AccountIsBot,
    created_at: CreatedAt<Account>,
    deleted_at: Option<DeletedAt<Account>>,
}

#[derive(Debug, Clone, Eq, PartialEq, Nameln, Serialize, Deserialize)]
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
        id: AccountId,
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
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(KnownEventVersion::Nothing),
        )
    }

    pub fn update(id: AccountId, is_bot: AccountIsBot) -> CommandEnvelope<AccountEvent, Account> {
        let event = AccountEvent::Updated { is_bot };
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }

    pub fn delete(id: AccountId) -> CommandEnvelope<AccountEvent, Account> {
        let event = AccountEvent::Deleted;
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }
}

impl EventApplier for Account {
    type Event = AccountEvent;
    const ENTITY_NAME: &'static str = "Account";

    fn apply(
        entity: &mut Option<Self>,
        event: EventEnvelope<Self::Event, Self>,
    ) -> error_stack::Result<(), KernelError> {
        match event.event {
            AccountEvent::Created {
                name,
                private_key,
                public_key,
                is_bot,
            } => {
                if let Some(entity) = entity {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::already_exists(entity)));
                }
                *entity = Some(Account {
                    id: AccountId::new(event.id.raw_id()),
                    name,
                    private_key,
                    public_key,
                    is_bot,
                    created_at: CreatedAt::now(),
                    deleted_at: None,
                });
            }
            AccountEvent::Updated { is_bot } => {
                if let Some(entity) = entity {
                    entity.is_bot = is_bot;
                } else {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
            }
            AccountEvent::Deleted => {
                if let Some(entity) = entity {
                    entity.deleted_at = Some(DeletedAt::now());
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
        Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
        CreatedAt, EventEnvelope, EventVersion,
    };
    use crate::event::EventApplier;
    use uuid::Uuid;

    #[test]
    fn create_account() {
        let id = AccountId::new(Uuid::now_v7());
        let name = AccountName::new("test");
        let private_key = AccountPrivateKey::new("private_key".to_string());
        let public_key = AccountPublicKey::new("public_key".to_string());
        let is_bot = AccountIsBot::new(false);
        let event = Account::create(
            id.clone(),
            name.clone(),
            private_key.clone(),
            public_key.clone(),
            is_bot.clone(),
        );
        let envelope = EventEnvelope::new(
            event.id().clone().into(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut account = None;
        Account::apply(&mut account, envelope).unwrap();
        assert!(account.is_some());
        let account = account.unwrap();
        assert_eq!(account.id(), &id);
        assert_eq!(account.name(), &name);
        assert_eq!(account.private_key(), &private_key);
        assert_eq!(account.public_key(), &public_key);
        assert_eq!(account.is_bot(), &is_bot);
    }

    #[test]
    #[should_panic]
    fn create_exist_account() {
        let id = AccountId::new(Uuid::now_v7());
        let name = AccountName::new("test");
        let private_key = AccountPrivateKey::new("private_key".to_string());
        let public_key = AccountPublicKey::new("public_key".to_string());
        let is_bot = AccountIsBot::new(false);
        let account = Account::new(
            id.clone(),
            name.clone(),
            private_key.clone(),
            public_key.clone(),
            is_bot.clone(),
            CreatedAt::now(),
            None,
        );
        let event = Account::create(
            id.clone(),
            name.clone(),
            private_key.clone(),
            public_key.clone(),
            is_bot.clone(),
        );
        let envelope = EventEnvelope::new(
            event.id().clone().into(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut account = Some(account);
        Account::apply(&mut account, envelope).unwrap();
    }

    #[test]
    fn update_account() {
        let id = AccountId::new(Uuid::now_v7());
        let name = AccountName::new("test");
        let private_key = AccountPrivateKey::new("private_key".to_string());
        let public_key = AccountPublicKey::new("public_key".to_string());
        let is_bot = AccountIsBot::new(false);
        let account = Account::new(
            id.clone(),
            name.clone(),
            private_key.clone(),
            public_key.clone(),
            is_bot.clone(),
            CreatedAt::now(),
            None,
        );
        let event = Account::update(id.clone(), AccountIsBot::new(true));
        let envelope = EventEnvelope::new(
            event.id().clone().into(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut account = Some(account);
        Account::apply(&mut account, envelope).unwrap();
        let account = account.unwrap();
        assert_eq!(account.is_bot(), &AccountIsBot::new(true));
    }

    #[test]
    #[should_panic]
    fn update_not_exist_account() {
        let id = AccountId::new(Uuid::now_v7());
        let event = Account::update(id.clone(), AccountIsBot::new(true));
        let envelope = EventEnvelope::new(
            event.id().clone().into(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut account = None;
        Account::apply(&mut account, envelope).unwrap();
    }

    #[test]
    fn delete_account() {
        let id = AccountId::new(Uuid::now_v7());
        let name = AccountName::new("test");
        let private_key = AccountPrivateKey::new("private_key".to_string());
        let public_key = AccountPublicKey::new("public_key".to_string());
        let is_bot = AccountIsBot::new(false);
        let account = Account::new(
            id.clone(),
            name.clone(),
            private_key.clone(),
            public_key.clone(),
            is_bot.clone(),
            CreatedAt::now(),
            None,
        );
        let event = Account::delete(id.clone());
        let envelope = EventEnvelope::new(
            event.id().clone().into(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut account = Some(account);
        Account::apply(&mut account, envelope).unwrap();
        let account = account.unwrap();
        assert!(account.deleted_at().is_some());
    }

    #[test]
    #[should_panic]
    fn delete_not_exist_account() {
        let id = AccountId::new(Uuid::now_v7());
        let event = Account::delete(id.clone());
        let envelope = EventEnvelope::new(
            event.id().clone().into(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut account = None;
        Account::apply(&mut account, envelope).unwrap();
    }
}
