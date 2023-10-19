use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Serialize, Deserialize)]
pub struct AccountHost(String);

impl AccountHost {
    pub fn new(host: impl Into<String>) -> Self {
        Self(host.into())
    }
}
