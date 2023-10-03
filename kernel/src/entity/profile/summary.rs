use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Summary(String);

impl Summary {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl AsRef<str> for Summary {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<Summary> for String {
    fn from(value: Summary) -> Self {
        value.0
    }
}
