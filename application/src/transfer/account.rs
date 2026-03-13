use kernel::prelude::entity::{Account, AccountStatus};
use time::OffsetDateTime;

#[derive(Debug)]
pub struct AccountDto {
    pub nanoid: String,
    pub name: String,
    pub public_key: String,
    pub is_bot: bool,
    pub created_at: OffsetDateTime,
    pub moderation: Option<ModerationDto>,
}

#[derive(Debug)]
pub enum ModerationDto {
    Suspended {
        reason: String,
        suspended_at: OffsetDateTime,
        expires_at: Option<OffsetDateTime>,
    },
    Banned {
        reason: String,
        banned_at: OffsetDateTime,
    },
}

impl From<Account> for AccountDto {
    fn from(account: Account) -> Self {
        let moderation = match account.status() {
            AccountStatus::Active => None,
            AccountStatus::Suspended {
                reason,
                suspended_at,
                expires_at,
            } => Some(ModerationDto::Suspended {
                reason: reason.clone(),
                suspended_at: *suspended_at,
                expires_at: *expires_at,
            }),
            AccountStatus::Banned { reason, banned_at } => Some(ModerationDto::Banned {
                reason: reason.clone(),
                banned_at: *banned_at,
            }),
        };
        Self {
            nanoid: account.nanoid().as_ref().to_string(),
            name: account.name().as_ref().to_string(),
            public_key: account.public_key().as_ref().to_string(),
            is_bot: *account.is_bot().as_ref(),
            created_at: *account.created_at().as_ref(),
            moderation,
        }
    }
}
