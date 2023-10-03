use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Banner(String);

impl Banner {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl AsRef<str> for Banner {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<Banner> for String {
    fn from(value: Banner) -> Self {
        value.0
    }
}
