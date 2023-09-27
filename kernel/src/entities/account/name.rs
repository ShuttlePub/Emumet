use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountName(String);

impl AccountName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

impl From<AccountName> for String {
    fn from(name: AccountName) -> Self {
        name.0
    }
}

impl AsRef<str> for AccountName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
