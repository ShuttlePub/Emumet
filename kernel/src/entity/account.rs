use destructure::Destructure;
use error_stack::Report;
use serde::Deserialize;
use serde::Serialize;
use vodca::{Nameln, Newln, References};

use crate::entity::{
    CommandEnvelope, CreatedAt, DeletedAt, EventEnvelope, EventId, EventVersion, KnownEventVersion,
    Nanoid,
};
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
    deleted_at: Option<DeletedAt<Account>>,
    version: EventVersion<Account>,
    nanoid: Nanoid<Account>,
    created_at: CreatedAt<Account>,
}

#[derive(Debug, Clone, Eq, PartialEq, Nameln, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "snake_case")]
#[vodca(prefix = "account", snake_case)]
pub enum AccountEvent {
    Created {
        name: AccountName,
        private_key: AccountPrivateKey,
        public_key: AccountPublicKey,
        is_bot: AccountIsBot,
        nanoid: Nanoid<Account>,
    },
    Updated {
        is_bot: AccountIsBot,
    },
    Deleted,
}

impl Account {
    pub fn update(
        id: AccountId,
        is_bot: AccountIsBot,
        current_version: EventVersion<Account>,
    ) -> CommandEnvelope<AccountEvent, Account> {
        let event = AccountEvent::Updated { is_bot };
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(KnownEventVersion::Prev(current_version)),
        )
    }

    pub fn delete(
        id: AccountId,
        current_version: EventVersion<Account>,
    ) -> CommandEnvelope<AccountEvent, Account> {
        let event = AccountEvent::Deleted;
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(KnownEventVersion::Prev(current_version)),
        )
    }
}

impl PartialOrd for Account {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Account {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id.cmp(&other.id)
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
                nanoid: nano_id,
            } => {
                if let Some(entity) = entity {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::already_exists(entity)));
                }
                let created_at = if let Some(timestamp) = event.id.as_ref().get_timestamp() {
                    CreatedAt::try_from(timestamp)?
                } else {
                    CreatedAt::now()
                };
                *entity = Some(Account {
                    id: AccountId::new(event.id),
                    name,
                    private_key,
                    public_key,
                    is_bot,
                    deleted_at: None,
                    version: event.version,
                    nanoid: nano_id,
                    created_at,
                });
            }
            AccountEvent::Updated { is_bot } => {
                if let Some(entity) = entity {
                    entity.is_bot = is_bot;
                    entity.version = event.version;
                } else {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
            }
            AccountEvent::Deleted => {
                if entity.is_some() {
                    *entity = None;
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
        Account, AccountEvent, AccountId, AccountIsBot, AccountName, AccountPrivateKey,
        AccountPublicKey, CreatedAt, EventEnvelope, EventId, EventVersion, Nanoid,
    };
    use crate::event::EventApplier;
    use crate::KernelError;
    use uuid::Uuid;

    #[test]
    fn create_account() {
        let id = AccountId::new(Uuid::now_v7());
        let name = AccountName::new("test");
        let private_key = AccountPrivateKey::new("private_key".to_string());
        let public_key = AccountPublicKey::new("public_key".to_string());
        let is_bot = AccountIsBot::new(false);
        let nano_id = Nanoid::default();
        let event = AccountEvent::Created {
            name: name.clone(),
            private_key: private_key.clone(),
            public_key: public_key.clone(),
            is_bot: is_bot.clone(),
            nanoid: nano_id.clone(),
        };
        let envelope = EventEnvelope::new(
            EventId::from(id.clone()),
            event,
            EventVersion::new(Uuid::now_v7()),
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
        assert_eq!(account.nanoid(), &nano_id);
    }

    #[test]
    fn create_exist_account() {
        let id = AccountId::new(Uuid::now_v7());
        let name = AccountName::new("test");
        let private_key = AccountPrivateKey::new("private_key".to_string());
        let public_key = AccountPublicKey::new("public_key".to_string());
        let is_bot = AccountIsBot::new(false);
        let nano_id = Nanoid::default();
        let account = Account::new(
            id.clone(),
            name.clone(),
            private_key.clone(),
            public_key.clone(),
            is_bot.clone(),
            None,
            EventVersion::new(Uuid::now_v7()),
            nano_id.clone(),
            CreatedAt::now(),
        );
        let event = AccountEvent::Created {
            name: name.clone(),
            private_key: private_key.clone(),
            public_key: public_key.clone(),
            is_bot: is_bot.clone(),
            nanoid: nano_id.clone(),
        };
        let envelope = EventEnvelope::new(
            EventId::from(id.clone()),
            event,
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = Some(account);
        assert!(Account::apply(&mut account, envelope)
            .is_err_and(|e| e.current_context() == &KernelError::Internal));
    }

    #[test]
    fn update_account() {
        let id = AccountId::new(Uuid::now_v7());
        let name = AccountName::new("test");
        let private_key = AccountPrivateKey::new("private_key".to_string());
        let public_key = AccountPublicKey::new("public_key".to_string());
        let is_bot = AccountIsBot::new(false);
        let nano_id = Nanoid::default();
        let account = Account::new(
            id.clone(),
            name.clone(),
            private_key.clone(),
            public_key.clone(),
            is_bot.clone(),
            None,
            EventVersion::new(Uuid::now_v7()),
            nano_id.clone(),
            CreatedAt::now(),
        );
        let version = account.version().clone();
        let event = Account::update(id.clone(), AccountIsBot::new(true), version);
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = Some(account);
        Account::apply(&mut account, envelope.clone()).unwrap();
        let account = account.unwrap();
        assert_eq!(account.is_bot(), &AccountIsBot::new(true));
        assert_eq!(account.version(), &envelope.version);
        assert_eq!(account.nanoid(), &nano_id);
    }

    #[test]
    fn update_not_exist_account() {
        let id = AccountId::new(Uuid::now_v7());
        let version = EventVersion::new(Uuid::now_v7());
        let event = Account::update(id.clone(), AccountIsBot::new(true), version);
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = None;
        assert!(Account::apply(&mut account, envelope)
            .is_err_and(|e| e.current_context() == &KernelError::Internal));
    }

    #[test]
    fn delete_account() {
        let id = AccountId::new(Uuid::now_v7());
        let name = AccountName::new("test");
        let private_key = AccountPrivateKey::new("private_key".to_string());
        let public_key = AccountPublicKey::new("public_key".to_string());
        let is_bot = AccountIsBot::new(false);
        let nano_id = Nanoid::default();
        let account = Account::new(
            id.clone(),
            name.clone(),
            private_key.clone(),
            public_key.clone(),
            is_bot.clone(),
            None,
            EventVersion::new(Uuid::now_v7()),
            nano_id.clone(),
            CreatedAt::now(),
        );
        let version = account.version().clone();
        let event = Account::delete(id.clone(), version);
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = Some(account);
        Account::apply(&mut account, envelope.clone()).unwrap();
        assert!(account.is_none());
    }

    #[test]
    fn delete_not_exist_account() {
        let id = AccountId::new(Uuid::now_v7());
        let version = EventVersion::new(Uuid::now_v7());
        let event = Account::delete(id.clone(), version);
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = None;
        assert!(Account::apply(&mut account, envelope)
            .is_err_and(|e| e.current_context() == &KernelError::Internal));
    }
}
