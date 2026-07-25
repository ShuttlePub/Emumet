use crate::entity::{AccountId, RemoteAccountId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MuteTargetId {
    Local(AccountId),
    Remote(RemoteAccountId),
}

impl From<AccountId> for MuteTargetId {
    fn from(id: AccountId) -> Self {
        Self::Local(id)
    }
}

impl From<RemoteAccountId> for MuteTargetId {
    fn from(id: RemoteAccountId) -> Self {
        Self::Remote(id)
    }
}
