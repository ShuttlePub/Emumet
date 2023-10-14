mod access_token;
mod host;
mod refresh_token;

use destructure::Destructure;
use serde::Deserialize;
use serde::Serialize;

pub use self::access_token::*;
pub use self::host::*;
pub use self::refresh_token::*;

use super::Id;

#[derive(Debug, Clone, Hash, Serialize, Deserialize, Destructure)]
pub struct StellarAccount {
    id: Id<StellarAccount>,
    host: AccountHost,
    access_token: AccessToken,
    refresh_token: RefreshToken,
}

impl StellarAccount {
    pub fn new(
        id: Id<StellarAccount>,
        host: AccountHost,
        access_token: AccessToken,
        refresh_token: RefreshToken,
    ) -> Self {
        Self {
            id,
            host,
            access_token,
            refresh_token,
        }
    }

    pub fn id(&self) -> &Id<StellarAccount> {
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
