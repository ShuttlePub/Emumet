use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Serialize, Deserialize)]
pub struct ProfileBanner(String);

impl ProfileBanner {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}
