use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccessToken(String);

impl AccessToken {
    fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }
}

impl From<AccessToken> for String {
    fn from(token: AccessToken) -> Self {
        token.0
    }
}

impl AsRef<str> for AccessToken {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
