use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, Hash, PartialEq, Eq, Fromln, AsRefln, Serialize, Deserialize)]
pub struct AccountDomain(String);

impl AccountDomain {
    pub fn new(domain: impl Into<String>) -> Self {
        Self(domain.into())
    }
}
