use crate::KernelError;
use serde::{Deserialize, Serialize};

use super::{Account, Id, RemoteAccount};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FollowAccount {
    Local(Id<Account>),
    Remote(Id<RemoteAccount>),
}

impl From<Id<Account>> for FollowAccount {
    fn from(id: Id<Account>) -> Self {
        Self::Local(id)
    }
}

impl From<Id<RemoteAccount>> for FollowAccount {
    fn from(id: Id<RemoteAccount>) -> Self {
        Self::Remote(id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Follow {
    id: Id<Follow>,
    source: FollowAccount,
    destination: FollowAccount,
}

impl Follow {
    pub fn new(
        id: Id<Follow>,
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

    pub fn id(&self) -> &Id<Follow> {
        &self.id
    }

    pub fn source(&self) -> &FollowAccount {
        &self.source
    }

    pub fn destination(&self) -> &FollowAccount {
        &self.destination
    }
}
