use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountHost(String);

impl AccountHost {
    pub fn new(host: impl Into<String>) -> Self {
        Self(host.into())
    }
}

impl From<AccountHost> for String {
    fn from(host: AccountHost) -> Self {
        host.0
    }
}

impl AsRef<str> for AccountHost {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
