mod id;
mod is_bot;
mod name;

use destructure::Destructure;
use serde::Deserialize;
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

pub use self::id::*;
pub use self::is_bot::*;
pub use self::name::*;

use super::StellarAccountId;
use super::time::CreatedAt;

#[derive(Debug, Clone, Hash, Serialize, Deserialize, Destructure)]
pub struct Account {
    id: AccountId,
    stellar_id: StellarAccountId,
    name: AccountName,
    is_bot: IsBot,
    created_at: CreatedAt<Account>
}

impl Account {
    pub fn new(
        id: impl Into<i64>,
        stellar_id: impl Into<Uuid>,
        name: impl Into<String>,
        is_bot: impl Into<bool>,
        created_at: impl Into<OffsetDateTime>,
    ) -> Self {
        Self {
            id: AccountId::new(id),
            stellar_id: StellarAccountId::new(stellar_id),
            name: AccountName::new(name),
            is_bot: IsBot::new(is_bot),
            created_at: CreatedAt::new(created_at),
        }
    }

    pub fn id(&self) -> &AccountId {
        &self.id
    }

    pub fn stellar_id(&self) -> &StellarAccountId {
        &self.stellar_id
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
