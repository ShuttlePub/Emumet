mod id;

use crate::KernelError;
use error_stack::ResultExt;
use serde::{Deserialize, Serialize};
use vodca::References;

pub use self::id::*;

use super::{AccountId, RemoteAccountId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FollowAccountId {
    Local(AccountId),
    Remote(RemoteAccountId),
}

impl FollowAccountId {
    pub fn new(
        local: Option<AccountId>,
        remote: Option<RemoteAccountId>,
    ) -> error_stack::Result<Self, KernelError> {
        match (local, remote) {
            (Some(local), None) => Ok(Self::Local(local)),
            (None, Some(remote)) => Ok(Self::Remote(remote)),
            (Some(local), Some(remote)) => Err(KernelError::Internal)
                .attach_printable(format!("local: {:?} and remote: {:?}", local, remote)),
            (None, None) => Err(KernelError::Internal)
                .attach_printable("local: None and remote: None".to_string()),
        }
    }
}

impl From<AccountId> for FollowAccountId {
    fn from(id: AccountId) -> Self {
        Self::Local(id)
    }
}

impl From<RemoteAccountId> for FollowAccountId {
    fn from(id: RemoteAccountId) -> Self {
        Self::Remote(id)
    }
}

#[derive(Debug, Clone, References, Serialize, Deserialize)]
pub struct Follow {
    id: FollowId,
    source: FollowAccountId,
    destination: FollowAccountId,
}

impl Follow {
    pub fn new(
        id: FollowId,
        source: FollowAccountId,
        destination: FollowAccountId,
    ) -> error_stack::Result<Self, KernelError> {
        match (source, destination) {
            (source @ FollowAccountId::Local(_), destination @ FollowAccountId::Local(_))
            | (source @ FollowAccountId::Remote(_), destination @ FollowAccountId::Remote(_)) => {
                Err(KernelError::Internal).attach_printable(format!(
                    "source: {:?}, destination: {:?}",
                    source, destination
                ))
            }
            (source, destination) => Ok(Self {
                id,
                source,
                destination,
            }),
        }
    }
}
