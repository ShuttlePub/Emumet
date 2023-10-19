mod id;

use crate::KernelError;
use serde::{Deserialize, Serialize};
use vodca::References;

pub use self::id::*;

use super::{AccountId, RemoteAccountId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FollowAccount {
    Local(AccountId),
    Remote(RemoteAccountId),
}

impl FollowAccount {
    pub fn new(
        local: Option<AccountId>,
        remote: Option<RemoteAccountId>,
    ) -> Result<Self, KernelError> {
        match (local, remote) {
            (Some(local), None) => Ok(Self::Local(local)),
            (None, Some(remote)) => Ok(Self::Remote(remote)),
            (Some(local), Some(remote)) => Err(KernelError::InvalidValue {
                method: "FollowAccount::new",
                value: format!("local: {:?} and remote: {:?}", local, remote),
            }),
            (None, None) => Err(KernelError::InvalidValue {
                method: "FollowAccount::new",
                value: "local: None and remote: None".to_string(),
            }),
        }
    }
}

impl From<AccountId> for FollowAccount {
    fn from(id: AccountId) -> Self {
        Self::Local(id)
    }
}

impl From<RemoteAccountId> for FollowAccount {
    fn from(id: RemoteAccountId) -> Self {
        Self::Remote(id)
    }
}

#[derive(Debug, Clone, References, Serialize, Deserialize)]
pub struct Follow {
    id: FollowId,
    source: FollowAccount,
    destination: FollowAccount,
}

impl Follow {
    pub fn new(
        id: FollowId,
        source: FollowAccount,
        destination: FollowAccount,
    ) -> Result<Self, KernelError> {
        match (source, destination) {
            (source @ FollowAccount::Local(_), destination @ FollowAccount::Local(_))
            | (source @ FollowAccount::Remote(_), destination @ FollowAccount::Remote(_)) => {
                Err(KernelError::InvalidValue {
                    method: "Follow::new",
                    value: format!("source: {:?}, destination: {:?}", source, destination),
                })
            }
            (source, destination) => Ok(Self {
                id,
                source,
                destination,
            }),
        }
    }
}
