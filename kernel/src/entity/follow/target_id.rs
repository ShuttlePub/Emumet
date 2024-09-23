use crate::entity::{AccountId, RemoteAccountId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum FollowTargetId {
    Local(AccountId),
    Remote(RemoteAccountId),
}

impl From<AccountId> for FollowTargetId {
    fn from(id: AccountId) -> Self {
        Self::Local(id)
    }
}

impl From<RemoteAccountId> for FollowTargetId {
    fn from(id: RemoteAccountId) -> Self {
        Self::Remote(id)
    }
}
