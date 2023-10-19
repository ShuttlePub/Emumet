mod domain;
mod id;
mod is_bot;
mod name;

use destructure::Destructure;
use serde::Deserialize;
use serde::Serialize;
use vodca::References;

pub use self::domain::*;
pub use self::id::*;
pub use self::is_bot::*;
pub use self::name::*;

use super::common::CreatedAt;

#[derive(Debug, Clone, Hash, References, Serialize, Deserialize, Destructure)]
pub struct Account {
    id: AccountId,
    domain: AccountDomain,
    name: AccountName,
    is_bot: AccountIsBot,
    created_at: CreatedAt<Account>,
}

impl Account {
    pub fn new(
        id: AccountId,
        domain: AccountDomain,
        name: AccountName,
        is_bot: AccountIsBot,
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
}
