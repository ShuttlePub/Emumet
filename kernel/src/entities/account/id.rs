use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountId(i64);

impl AccountId {
    pub fn new(id: impl Into<i64>) -> Self {
        Self(id.into())
    }
}

impl From<AccountId> for i64 {
    fn from(value: AccountId) -> Self {
        value.0
    }
}

impl AsRef<i64> for AccountId {
    fn as_ref(&self) -> &i64 {
        &self.0
    }
}
