use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDomain(String);

impl AccountDomain {
    pub fn new(domain: impl Into<String>) -> Self {
        Self(domain.into())
    }
}

impl From<AccountDomain> for String {
    fn from(domain: AccountDomain) -> Self {
        domain.0
    }
}

impl AsRef<str> for AccountDomain {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
