use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Icon(String);

impl Icon {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl AsRef<str> for Icon {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<Icon> for String {
    fn from(value: Icon) -> Self {
        value.0
    }
}
