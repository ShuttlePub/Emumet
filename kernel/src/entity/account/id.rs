use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Serialize, Deserialize)]
pub struct AccountId(i64);

impl AccountId {
    pub fn new(id: impl Into<i64>) -> Self {
        Self(id.into())
    }
}
