mod id;
mod access_token;
mod refresh_token;
mod host;

use destructure::Destructure;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

pub use self::id::*;
pub use self::access_token::*;
pub use self::refresh_token::*;
pub use self::host::*;

#[derive(Debug, Clone, Hash, Serialize, Deserialize, Destructure)]
pub struct StellarAccount {
    id: StellarAccountId,
    host: AccountHost,
    access_token: AccessToken,
    refresh_token: RefreshToken,
}

impl StellarAccount {
    pub fn new(
        id: impl Into<Uuid>,
        host: impl Into<String>,
        access_token: impl Into<String>,
        refresh_token: impl Into<String>,
    ) -> Self {
        Self {
            id: StellarAccountId::new(id),
            host: AccountHost::new(host),
            access_token: AccessToken::new(access_token),
            refresh_token: RefreshToken::new(refresh_token),
        }
    }

    pub fn id(&self) -> &StellarAccountId {
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
