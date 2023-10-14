mod domain;
mod is_bot;
mod name;

use destructure::Destructure;
use serde::Deserialize;
use serde::Serialize;
use time::OffsetDateTime;

pub use self::domain::*;
pub use self::is_bot::*;
pub use self::name::*;

use super::common::CreatedAt;
use super::Id;

#[derive(Debug, Clone, Hash, Serialize, Deserialize, Destructure)]
pub struct Account {
    id: Id<Account>,
    domain: AccountDomain,
    name: AccountName,
    is_bot: IsBot,
    created_at: CreatedAt<Account>,
}

impl Account {
    pub fn new(
        id: Id<Account>,
        domain: AccountDomain,
        name: AccountName,
        is_bot: IsBot,
        created_at: CreatedAt<Account>,
    ) -> Self {
        Self {
            id,
            domain,
            name,
            is_bot,
            created_at,
        }
    }

    pub fn id(&self) -> &Id<Account> {
        &self.id
    }

    pub fn domain(&self) -> &AccountDomain {
        &self.domain
    }

    pub fn name(&self) -> &AccountName {
        &self.name
    }

    pub fn is_bot(&self) -> &IsBot {
        &self.is_bot
    }

    pub fn created_at(&self) -> &CreatedAt<Account> {
        &self.created_at
    }
}
