use destructure::Destructure;
use error_stack::Report;
use serde::Deserialize;
use serde::Serialize;
use vodca::{Nameln, Newln, References};

use time::OffsetDateTime;

use crate::entity::{
    AuthAccountId, CommandEnvelope, CreatedAt, DeletedAt, EventEnvelope, EventId, EventVersion,
    KnownEventVersion, Nanoid,
};
use crate::event::EventApplier;
use crate::KernelError;

pub use self::id::*;
pub use self::is_bot::*;
pub use self::name::*;
pub use self::private_key::*;
pub use self::public_key::*;
pub use self::status::*;

mod id;
mod is_bot;
mod name;
mod private_key;
mod public_key;
mod status;

#[derive(
    Debug, Clone, Hash, Eq, PartialEq, References, Newln, Serialize, Deserialize, Destructure,
)]
pub struct Account {
    id: AccountId,
    name: AccountName,
    private_key: AccountPrivateKey,
    public_key: AccountPublicKey,
    is_bot: AccountIsBot,
    status: AccountStatus,
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
        auth_account_id: AuthAccountId,
    },
    Updated {
        is_bot: AccountIsBot,
    },
    #[serde(alias = "deleted")]
    Deactivated,
    Suspended {
        reason: String,
        #[serde(with = "time::serde::rfc3339")]
        suspended_at: OffsetDateTime,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "time::serde::rfc3339::option"
        )]
        expires_at: Option<OffsetDateTime>,
    },
    Unsuspended,
    Banned {
        reason: String,
        #[serde(with = "time::serde::rfc3339")]
        banned_at: OffsetDateTime,
    },
}

impl Account {
    pub fn create(
        id: AccountId,
        name: AccountName,
        private_key: AccountPrivateKey,
        public_key: AccountPublicKey,
        is_bot: AccountIsBot,
        nanoid: Nanoid<Account>,
        auth_account_id: AuthAccountId,
    ) -> CommandEnvelope<AccountEvent, Account> {
        let event = AccountEvent::Created {
            name,
            private_key,
            public_key,
            is_bot,
            nanoid,
            auth_account_id,
        };
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(KnownEventVersion::Nothing),
        )
    }

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

    pub fn deactivate(
        id: AccountId,
        current_version: EventVersion<Account>,
    ) -> CommandEnvelope<AccountEvent, Account> {
        let event = AccountEvent::Deactivated;
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(KnownEventVersion::Prev(current_version)),
        )
    }

    pub fn suspend(
        id: AccountId,
        reason: String,
        expires_at: Option<OffsetDateTime>,
        current_version: EventVersion<Account>,
    ) -> CommandEnvelope<AccountEvent, Account> {
        let event = AccountEvent::Suspended {
            reason,
            suspended_at: OffsetDateTime::now_utc(),
            expires_at,
        };
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(KnownEventVersion::Prev(current_version)),
        )
    }

    pub fn unsuspend(
        id: AccountId,
        current_version: EventVersion<Account>,
    ) -> CommandEnvelope<AccountEvent, Account> {
        let event = AccountEvent::Unsuspended;
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(KnownEventVersion::Prev(current_version)),
        )
    }

    pub fn ban(
        id: AccountId,
        reason: String,
        current_version: EventVersion<Account>,
    ) -> CommandEnvelope<AccountEvent, Account> {
        let event = AccountEvent::Banned {
            reason,
            banned_at: OffsetDateTime::now_utc(),
        };
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
                auth_account_id: _,
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
                    status: AccountStatus::Active,
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
            AccountEvent::Deactivated => {
                if let Some(account) = entity {
                    if account.deleted_at.is_some() {
                        return Err(Report::new(KernelError::Internal)
                            .attach_printable("Account is already deactivated"));
                    }
                    account.deleted_at = Some(DeletedAt::now());
                    account.version = event.version;
                } else {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
            }
            AccountEvent::Suspended {
                reason,
                suspended_at,
                expires_at,
            } => {
                if let Some(account) = entity {
                    if !account.status.is_active() {
                        return Err(Report::new(KernelError::Internal)
                            .attach_printable("Account is not active"));
                    }
                    if account.deleted_at.is_some() {
                        return Err(Report::new(KernelError::Internal)
                            .attach_printable("Account is deactivated"));
                    }
                    account.status = AccountStatus::Suspended {
                        reason,
                        suspended_at,
                        expires_at,
                    };
                    account.version = event.version;
                } else {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
            }
            AccountEvent::Unsuspended => {
                if let Some(account) = entity {
                    if !account.status.is_suspended() {
                        return Err(Report::new(KernelError::Internal)
                            .attach_printable("Account is not suspended"));
                    }
                    account.status = AccountStatus::Active;
                    account.version = event.version;
                } else {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
            }
            AccountEvent::Banned { reason, banned_at } => {
                if let Some(account) = entity {
                    if account.status.is_banned() {
                        return Err(Report::new(KernelError::Internal)
                            .attach_printable("Account is already banned"));
                    }
                    if account.deleted_at.is_some() {
                        return Err(Report::new(KernelError::Internal)
                            .attach_printable("Account is deactivated"));
                    }
                    account.status = AccountStatus::Banned { reason, banned_at };
                    account.version = event.version;
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
        AccountPublicKey, AuthAccountId, CreatedAt, EventEnvelope, EventId, EventVersion, Nanoid,
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
            auth_account_id: AuthAccountId::new(Uuid::now_v7()),
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
            Default::default(),
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
            auth_account_id: AuthAccountId::new(Uuid::now_v7()),
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
            Default::default(),
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
    fn deactivate_account() {
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
            Default::default(),
            None,
            EventVersion::new(Uuid::now_v7()),
            nano_id.clone(),
            CreatedAt::now(),
        );
        let version = account.version().clone();
        let event = Account::deactivate(id.clone(), version);
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = Some(account);
        Account::apply(&mut account, envelope.clone()).unwrap();
        assert!(account.is_some());
        let account = account.unwrap();
        assert!(account.deleted_at().is_some());
    }

    #[test]
    fn deactivate_already_deactivated_account() {
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
            Default::default(),
            None,
            EventVersion::new(Uuid::now_v7()),
            nano_id.clone(),
            CreatedAt::now(),
        );
        let version = account.version().clone();
        let event = Account::deactivate(id.clone(), version);
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = Some(account);
        Account::apply(&mut account, envelope).unwrap();
        assert!(account.is_some());
        assert!(account.as_ref().unwrap().deleted_at().is_some());

        // Second deactivation should fail
        let version2 = account.as_ref().unwrap().version().clone();
        let event2 = Account::deactivate(id.clone(), version2);
        let envelope2 = EventEnvelope::new(
            event2.id().clone(),
            event2.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        assert!(Account::apply(&mut account, envelope2)
            .is_err_and(|e| e.current_context() == &KernelError::Internal));
    }

    #[test]
    fn deactivate_not_exist_account() {
        let id = AccountId::new(Uuid::now_v7());
        let version = EventVersion::new(Uuid::now_v7());
        let event = Account::deactivate(id.clone(), version);
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = None;
        assert!(Account::apply(&mut account, envelope)
            .is_err_and(|e| e.current_context() == &KernelError::Internal));
    }

    fn make_active_account() -> (AccountId, Account) {
        let id = AccountId::new(Uuid::now_v7());
        let account = Account::new(
            id.clone(),
            AccountName::new("test"),
            AccountPrivateKey::new("private_key".to_string()),
            AccountPublicKey::new("public_key".to_string()),
            AccountIsBot::new(false),
            Default::default(),
            None,
            EventVersion::new(Uuid::now_v7()),
            Nanoid::default(),
            CreatedAt::now(),
        );
        (id, account)
    }

    #[test]
    fn suspend_account() {
        let (id, account) = make_active_account();
        let version = account.version().clone();
        let event = Account::suspend(id, "spam".into(), None, version);
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = Some(account);
        Account::apply(&mut account, envelope).unwrap();
        let account = account.unwrap();
        assert!(account.status().is_suspended());
    }

    #[test]
    fn suspend_already_suspended_account() {
        use crate::entity::AccountStatus;
        use time::OffsetDateTime;

        let id = AccountId::new(Uuid::now_v7());
        let account = Account::new(
            id.clone(),
            AccountName::new("test"),
            AccountPrivateKey::new("private_key".to_string()),
            AccountPublicKey::new("public_key".to_string()),
            AccountIsBot::new(false),
            AccountStatus::Suspended {
                reason: "spam".into(),
                suspended_at: OffsetDateTime::now_utc(),
                expires_at: None,
            },
            None,
            EventVersion::new(Uuid::now_v7()),
            Nanoid::default(),
            CreatedAt::now(),
        );
        let version = account.version().clone();
        let event = Account::suspend(id, "another reason".into(), None, version);
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = Some(account);
        assert!(Account::apply(&mut account, envelope)
            .is_err_and(|e| e.current_context() == &KernelError::Internal));
    }

    #[test]
    fn unsuspend_account() {
        use crate::entity::AccountStatus;
        use time::OffsetDateTime;

        let id = AccountId::new(Uuid::now_v7());
        let account = Account::new(
            id.clone(),
            AccountName::new("test"),
            AccountPrivateKey::new("private_key".to_string()),
            AccountPublicKey::new("public_key".to_string()),
            AccountIsBot::new(false),
            AccountStatus::Suspended {
                reason: "spam".into(),
                suspended_at: OffsetDateTime::now_utc(),
                expires_at: None,
            },
            None,
            EventVersion::new(Uuid::now_v7()),
            Nanoid::default(),
            CreatedAt::now(),
        );
        let version = account.version().clone();
        let event = Account::unsuspend(id, version);
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = Some(account);
        Account::apply(&mut account, envelope).unwrap();
        let account = account.unwrap();
        assert!(account.status().is_active());
    }

    #[test]
    fn unsuspend_active_account() {
        let (id, account) = make_active_account();
        let version = account.version().clone();
        let event = Account::unsuspend(id, version);
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = Some(account);
        assert!(Account::apply(&mut account, envelope)
            .is_err_and(|e| e.current_context() == &KernelError::Internal));
    }

    #[test]
    fn ban_account() {
        let (id, account) = make_active_account();
        let version = account.version().clone();
        let event = Account::ban(id, "violation".into(), version);
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = Some(account);
        Account::apply(&mut account, envelope).unwrap();
        let account = account.unwrap();
        assert!(account.status().is_banned());
    }

    #[test]
    fn ban_already_banned_account() {
        use crate::entity::AccountStatus;
        use time::OffsetDateTime;

        let id = AccountId::new(Uuid::now_v7());
        let account = Account::new(
            id.clone(),
            AccountName::new("test"),
            AccountPrivateKey::new("private_key".to_string()),
            AccountPublicKey::new("public_key".to_string()),
            AccountIsBot::new(false),
            AccountStatus::Banned {
                reason: "violation".into(),
                banned_at: OffsetDateTime::now_utc(),
            },
            None,
            EventVersion::new(Uuid::now_v7()),
            Nanoid::default(),
            CreatedAt::now(),
        );
        let version = account.version().clone();
        let event = Account::ban(id, "another".into(), version);
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = Some(account);
        assert!(Account::apply(&mut account, envelope)
            .is_err_and(|e| e.current_context() == &KernelError::Internal));
    }
}
