use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Serialize, Deserialize)]
pub struct ImageHash(String);

impl ImageHash {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}
