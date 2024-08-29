use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Serialize, Deserialize)]
pub struct ImageId(Uuid);

impl ImageId {
    pub fn new(value: impl Into<Uuid>) -> Self {
        Self(value.into())
    }
}
