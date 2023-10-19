use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Serialize, Deserialize)]
pub struct StellarAccountRefreshToken(String);

impl StellarAccountRefreshToken {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }
}
