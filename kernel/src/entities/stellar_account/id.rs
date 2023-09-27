use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct StellarAccountId(Uuid);

impl StellarAccountId {
    pub fn new(id: impl Into<Uuid>) -> Self {
        Self(id.into())
    }
}

impl From<StellarAccountId> for Uuid {
    fn from(value: StellarAccountId) -> Self {
        value.0
    }
}

impl AsRef<Uuid> for StellarAccountId {
    fn as_ref(&self) -> &Uuid {
        &self.0
    }
}
