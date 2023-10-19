use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Serialize, Deserialize)]
pub struct AccountName(String);

impl AccountName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}
