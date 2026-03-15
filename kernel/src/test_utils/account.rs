use crate::entity::{
    Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
    AccountStatus, CreatedAt, DeletedAt, EventVersion, Nanoid,
};

use super::{DEFAULT_ACCOUNT_NAME, DEFAULT_PRIVATE_KEY, DEFAULT_PUBLIC_KEY};

pub struct AccountBuilder {
    id: Option<AccountId>,
    name: Option<AccountName>,
    private_key: Option<AccountPrivateKey>,
    public_key: Option<AccountPublicKey>,
    is_bot: Option<AccountIsBot>,
    status: Option<AccountStatus>,
    deleted_at: Option<Option<DeletedAt<Account>>>,
    version: Option<EventVersion<Account>>,
    nanoid: Option<Nanoid<Account>>,
    created_at: Option<CreatedAt<Account>>,
}

impl Default for AccountBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AccountBuilder {
    pub fn new() -> Self {
        Self {
            id: None,
            name: None,
            private_key: None,
            public_key: None,
            is_bot: None,
            status: None,
            deleted_at: None,
            version: None,
            nanoid: None,
            created_at: None,
        }
    }

    pub fn id(mut self, id: AccountId) -> Self {
        self.id = Some(id);
        self
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(AccountName::new(name));
        self
    }

    pub fn private_key(mut self, key: impl Into<String>) -> Self {
        self.private_key = Some(AccountPrivateKey::new(key));
        self
    }

    pub fn public_key(mut self, key: impl Into<String>) -> Self {
        self.public_key = Some(AccountPublicKey::new(key));
        self
    }

    pub fn is_bot(mut self, is_bot: bool) -> Self {
        self.is_bot = Some(AccountIsBot::new(is_bot));
        self
    }

    pub fn status(mut self, status: AccountStatus) -> Self {
        self.status = Some(status);
        self
    }

    pub fn deleted_at(mut self, deleted_at: Option<DeletedAt<Account>>) -> Self {
        self.deleted_at = Some(deleted_at);
        self
    }

    pub fn version(mut self, version: EventVersion<Account>) -> Self {
        self.version = Some(version);
        self
    }

    pub fn nanoid(mut self, nanoid: Nanoid<Account>) -> Self {
        self.nanoid = Some(nanoid);
        self
    }

    pub fn created_at(mut self, created_at: CreatedAt<Account>) -> Self {
        self.created_at = Some(created_at);
        self
    }

    pub fn build(self) -> Account {
        crate::ensure_generator_initialized();
        Account::new(
            self.id.unwrap_or_default(),
            self.name
                .unwrap_or_else(|| AccountName::new(DEFAULT_ACCOUNT_NAME)),
            self.private_key
                .unwrap_or_else(|| AccountPrivateKey::new(DEFAULT_PRIVATE_KEY)),
            self.public_key
                .unwrap_or_else(|| AccountPublicKey::new(DEFAULT_PUBLIC_KEY)),
            self.is_bot.unwrap_or_else(|| AccountIsBot::new(false)),
            self.status.unwrap_or_default(),
            self.deleted_at.unwrap_or(None),
            self.version.unwrap_or_default(),
            self.nanoid.unwrap_or_default(),
            self.created_at.unwrap_or_else(CreatedAt::now),
        )
    }
}
