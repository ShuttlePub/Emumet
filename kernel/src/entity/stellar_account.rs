mod access_token;
mod host;
mod id;
mod refresh_token;

use destructure::Destructure;
use serde::Deserialize;
use serde::Serialize;
use vodca::References;

pub use self::access_token::*;
pub use self::host::*;
pub use self::id::*;
pub use self::refresh_token::*;

#[derive(Debug, Clone, Hash, References, Serialize, Deserialize, Destructure)]
pub struct StellarAccount {
    id: StellarAccountId,
    host: AccountHost,
    access_token: AccessToken,
    refresh_token: StellarAccountRefreshToken,
}

impl StellarAccount {
    pub fn new(
        id: StellarAccountId,
        host: AccountHost,
        access_token: AccessToken,
        refresh_token: StellarAccountRefreshToken,
    ) -> Self {
        Self {
            id,
            host,
            access_token,
            refresh_token,
        }
    }
}
