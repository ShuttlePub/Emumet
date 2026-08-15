use kernel::prelude::entity::{Account, AccountStatus, FieldAction};
use time::OffsetDateTime;

#[derive(Debug)]
pub struct CreateAccountDto {
    pub name: String,
    pub is_bot: bool,
}

#[derive(Debug)]
pub struct UpdateAccountDto {
    pub account_nanoid: String,
    pub is_bot: FieldAction<bool>,
    pub display_name: FieldAction<String>,
    pub summary: FieldAction<String>,
    pub icon_url: FieldAction<String>,
    pub banner_url: FieldAction<String>,
    pub fields: Option<Vec<AccountFieldDto>>,
}

#[derive(Debug)]
pub struct AccountDto {
    pub nanoid: String,
    pub name: String,
    pub is_bot: bool,
    pub created_at: OffsetDateTime,
    pub moderation: Option<ModerationDto>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AccountFieldDto {
    pub label: String,
    pub content: String,
}

#[derive(Debug)]
pub struct AccountDetailDto {
    pub nanoid: String,
    pub name: String,
    pub display_name: Option<String>,
    pub summary: Option<String>,
    pub icon_url: Option<String>,
    pub banner_url: Option<String>,
    pub is_bot: bool,
    pub fields: Vec<AccountFieldDto>,
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
            is_bot: *account.is_bot().as_ref(),
            created_at: *account.created_at().as_ref(),
            moderation,
        }
    }
}

impl AccountDto {
    pub fn into_detail(
        self,
        display_name: Option<String>,
        summary: Option<String>,
        icon_url: Option<String>,
        banner_url: Option<String>,
        fields: Vec<AccountFieldDto>,
    ) -> AccountDetailDto {
        AccountDetailDto {
            nanoid: self.nanoid,
            name: self.name,
            display_name,
            summary,
            icon_url,
            banner_url,
            is_bot: self.is_bot,
            fields,
            created_at: self.created_at,
            moderation: self.moderation,
        }
    }
}
