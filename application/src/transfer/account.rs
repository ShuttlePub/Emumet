use kernel::prelude::entity::Account;
use time::OffsetDateTime;

#[derive(Debug)]
pub struct AccountDto {
    pub nanoid: String,
    pub name: String,
    pub public_key: String,
    pub is_bot: bool,
    pub created_at: OffsetDateTime,
}

impl From<Account> for AccountDto {
    fn from(account: Account) -> Self {
        Self {
            nanoid: account.nanoid().as_ref().to_string(),
            name: account.name().as_ref().to_string(),
            public_key: account.public_key().as_ref().to_string(),
            is_bot: *account.is_bot().as_ref(),
            created_at: *account.created_at().as_ref(),
        }
    }
}
