use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Serialize, Deserialize)]
pub struct RemoteAccountId(Uuid);

impl RemoteAccountId {
    pub fn new(id: impl Into<Uuid>) -> Self {
        Self(id.into())
    }
}
