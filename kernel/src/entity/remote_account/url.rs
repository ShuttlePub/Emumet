use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Serialize, Deserialize)]
pub struct RemoteAccountUrl(String);

impl RemoteAccountUrl {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}
