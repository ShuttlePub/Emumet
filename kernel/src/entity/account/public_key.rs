use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Serialize, Deserialize)]
pub struct AccountPublicKey(String);

impl AccountPublicKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }
}
