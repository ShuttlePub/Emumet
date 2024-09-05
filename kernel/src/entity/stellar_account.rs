mod access_token;
mod client_id;
mod host;
mod id;
mod refresh_token;

use crate::entity::{CommandEnvelope, ExpectedEventVersion};
use destructure::Destructure;
use serde::Deserialize;
use serde::Serialize;
use vodca::{Newln, References};

pub use self::access_token::*;
pub use self::client_id::*;
pub use self::host::*;
pub use self::id::*;
pub use self::refresh_token::*;

#[derive(Debug, Clone, Hash, References, Newln, Serialize, Deserialize, Destructure)]
pub struct StellarAccount {
    id: StellarAccountId,
    host: StellarAccountHost,
    client_id: StellarAccountClientId,
    access_token: StellarAccountAccessToken,
    refresh_token: StellarAccountRefreshToken,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "snake_case")]
pub enum StellarAccountEvent {
    Created {
        host: StellarAccountHost,
        client_id: StellarAccountClientId,
        access_token: StellarAccountAccessToken,
        refresh_token: StellarAccountRefreshToken,
    },
    Updated {
        access_token: StellarAccountAccessToken,
        refresh_token: StellarAccountRefreshToken,
    },
    Deleted,
}

impl StellarAccount {
    pub fn create(
        host: StellarAccountHost,
        client_id: StellarAccountClientId,
        access_token: StellarAccountAccessToken,
        refresh_token: StellarAccountRefreshToken,
    ) -> CommandEnvelope<StellarAccountEvent, StellarAccount> {
        let event = StellarAccountEvent::Created {
            host,
            client_id,
            access_token,
            refresh_token,
        };
        CommandEnvelope::new(event, Some(ExpectedEventVersion::Nothing))
    }

    pub fn update(
        access_token: StellarAccountAccessToken,
        refresh_token: StellarAccountRefreshToken,
    ) -> CommandEnvelope<StellarAccountEvent, StellarAccount> {
        let event = StellarAccountEvent::Updated {
            access_token,
            refresh_token,
        };
        CommandEnvelope::new(event, None)
    }

    pub fn delete() -> CommandEnvelope<StellarAccountEvent, StellarAccount> {
        let event = StellarAccountEvent::Deleted;
        CommandEnvelope::new(event, None)
    }
}
