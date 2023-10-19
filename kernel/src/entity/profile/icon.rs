use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Serialize, Deserialize)]
pub struct ProfileIcon(String);

impl ProfileIcon {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}
