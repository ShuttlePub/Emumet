use kernel::prelude::entity::{
    Account, AccountId, AccountIsBot, AccountName, AccountStatus, CreatedAt, DeletedAt,
    EventVersion, Nanoid,
};
use sqlx::types::time::OffsetDateTime;

#[derive(sqlx::FromRow)]
pub(super) struct AccountRow {
    id: i64,
    name: String,
    is_bot: bool,
    deleted_at: Option<OffsetDateTime>,
    version: i64,
    nanoid: String,
    created_at: OffsetDateTime,
    suspended_at: Option<OffsetDateTime>,
    suspend_expires_at: Option<OffsetDateTime>,
    suspend_reason: Option<String>,
    banned_at: Option<OffsetDateTime>,
    ban_reason: Option<String>,
}

/// Convert an AccountRow into an Account.
///
/// When `check_suspend_expiry` is `false` (used for filtered queries where SQL already
/// excludes expired suspensions), suspended_at is trusted as-is.
/// When `true` (used for unfiltered queries), Rust-side expiry check is performed.
pub(super) fn account_from_row(value: AccountRow, check_suspend_expiry: bool) -> Account {
    let status = if let (Some(banned_at), Some(reason)) = (value.banned_at, value.ban_reason) {
        AccountStatus::Banned { reason, banned_at }
    } else if let (Some(suspended_at), Some(reason)) =
        (value.suspended_at, value.suspend_reason.clone())
    {
        if check_suspend_expiry {
            if let Some(expires_at) = value.suspend_expires_at {
                if expires_at <= OffsetDateTime::now_utc() {
                    AccountStatus::Active
                } else {
                    AccountStatus::Suspended {
                        reason,
                        suspended_at,
                        expires_at: Some(expires_at),
                    }
                }
            } else {
                AccountStatus::Suspended {
                    reason,
                    suspended_at,
                    expires_at: None,
                }
            }
        } else {
            AccountStatus::Suspended {
                reason,
                suspended_at,
                expires_at: value.suspend_expires_at,
            }
        }
    } else {
        AccountStatus::Active
    };

    Account::new(
        AccountId::new(value.id),
        AccountName::new(value.name),
        AccountIsBot::new(value.is_bot),
        status,
        value.deleted_at.map(DeletedAt::new),
        EventVersion::new(value.version),
        Nanoid::new(value.nanoid),
        CreatedAt::new(value.created_at),
    )
}

impl From<AccountRow> for Account {
    fn from(value: AccountRow) -> Self {
        account_from_row(value, false)
    }
}

pub struct PostgresAccountReadModel;
