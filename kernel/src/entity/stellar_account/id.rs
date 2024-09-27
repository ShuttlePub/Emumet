use crate::entity::{EventId, StellarAccount, StellarAccountEvent};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vodca::{AsRefln, Fromln, Newln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct StellarAccountId(Uuid);

impl From<StellarAccountId> for EventId<StellarAccountEvent, StellarAccount> {
    fn from(stellar_account_id: StellarAccountId) -> Self {
        EventId::new(stellar_account_id.0)
    }
}
