use serde::{Deserialize, Serialize};

use super::{Account, Id, RemoteAccount};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FollowAccount {
    Local(Id<Account>),
    Remote(Id<RemoteAccount>),
}

impl Into<FollowAccount> for Id<Account> {
    fn into(self) -> FollowAccount {
        FollowAccount::Local(self)
    }
}

impl Into<FollowAccount> for Id<RemoteAccount> {
    fn into(self) -> FollowAccount {
        FollowAccount::Remote(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Follow {
    id: Id<Follow>,
    source: FollowAccount,
    destination: FollowAccount,
}

impl Follow {
    pub fn new(id: Id<Follow>, source: FollowAccount, destination: FollowAccount) -> Self {
        Self {
            id,
            source,
            destination,
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
