use error_stack::Report;
use kernel::prelude::entity::{
    Account, AccountId, AccountIsBot, AccountKind, AccountName, AccountStatus, CreatedAt,
    DeletedAt, EventVersion, Nanoid,
};
use kernel::KernelError;
use sqlx::types::time::OffsetDateTime;

#[derive(sqlx::FromRow)]
pub(super) struct AccountRow {
    id: i64,
    name: String,
    is_bot: bool,
    kind: String,
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
pub(super) fn account_from_row(
    value: AccountRow,
    check_suspend_expiry: bool,
) -> error_stack::Result<Account, KernelError> {
    let kind = match value.kind.as_str() {
        "personal" => AccountKind::Personal,
        "organization" => AccountKind::Organization,
        unknown => {
            return Err(Report::new(KernelError::Internal)
                .attach_printable(format!("Unknown account kind: {unknown}")))
        }
    };
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

    Ok(Account::new(
        AccountId::new(value.id),
        AccountName::new(value.name),
        AccountIsBot::new(value.is_bot),
        kind,
        status,
        value.deleted_at.map(DeletedAt::new),
        EventVersion::new(value.version),
        Nanoid::new(value.nanoid),
        CreatedAt::new(value.created_at),
    ))
}

impl TryFrom<AccountRow> for Account {
    type Error = Report<KernelError>;

    fn try_from(value: AccountRow) -> Result<Self, Self::Error> {
        account_from_row(value, false)
    }
}

pub struct PostgresAccountReadModel;
