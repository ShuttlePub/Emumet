mod id;
mod host;
mod access_token;
mod refresh_token;

use destructure::Destructure;
use serde::Deserialize;
use serde::Serialize;

pub use self::id::*;
pub use self::host::*;
pub use self::access_token::*;
pub use self::refresh_token::*;

#[derive(Debug, Clone, Hash, Serialize, Deserialize, Destructure)]
pub struct Account {
    id: AccountId,
    host: AccountHost,
    access_token: AccessToken,
    refresh_token: RefreshToken,
}

impl Account {
    pub fn new(
        id: impl Into<AccountId>,
        host: impl Into<AccountHost>,
        access_token: impl Into<AccessToken>,
        refresh_token: impl Into<RefreshToken>,
    ) -> Self {
        Self {
            id: id.into(),
            host: host.into(),
            access_token: access_token.into(),
            refresh_token: refresh_token.into(),
        }
    }

    pub fn id(&self) -> &AccountId {
        &self.id
    }

    pub fn host(&self) -> &AccountHost {
        &self.host
    }

    pub fn access_token(&self) -> &AccessToken {
        &self.access_token
    }

    pub fn refresh_token(&self) -> &RefreshToken {
        &self.refresh_token
    }
}
