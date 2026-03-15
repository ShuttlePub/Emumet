use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[derive(Default)]
pub enum AccountStatus {
    #[default]
    Active,
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
    Banned {
        reason: String,
        #[serde(with = "time::serde::rfc3339")]
        banned_at: OffsetDateTime,
    },
}

impl std::hash::Hash for AccountStatus {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        core::mem::discriminant(self).hash(state);
        match self {
            Self::Active => {}
            Self::Suspended {
                reason,
                suspended_at,
                expires_at,
            } => {
                reason.hash(state);
                suspended_at.unix_timestamp_nanos().hash(state);
                expires_at.map(|t| t.unix_timestamp_nanos()).hash(state);
            }
            Self::Banned { reason, banned_at } => {
                reason.hash(state);
                banned_at.unix_timestamp_nanos().hash(state);
            }
        }
    }
}

impl AccountStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn is_suspended(&self) -> bool {
        matches!(self, Self::Suspended { .. })
    }

    pub fn is_banned(&self) -> bool {
        matches!(self, Self::Banned { .. })
    }
}
