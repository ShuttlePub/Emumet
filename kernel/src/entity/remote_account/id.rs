use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Serialize, Deserialize)]
pub struct RemoteAccountId(i64);

impl RemoteAccountId {
    pub fn new(id: impl Into<i64>) -> Self {
        Self(id.into())
    }
}
