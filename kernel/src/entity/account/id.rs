use crate::entity::{Account, AccountEvent, EventId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vodca::{AsRefln, Fromln, Newln};

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    Fromln,
    AsRefln,
    Newln,
    Serialize,
    Deserialize,
)]
pub struct AccountId(Uuid);

impl Default for AccountId {
    fn default() -> Self {
        AccountId(Uuid::now_v7())
    }
}

impl From<AccountId> for EventId<AccountEvent, Account> {
    fn from(account_id: AccountId) -> Self {
        EventId::new(account_id.0)
    }
}
