use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Content(String);

impl Content {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl AsRef<str> for Content {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<Content> for String {
    fn from(value: Content) -> Self {
        value.0
    }
}
