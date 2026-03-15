use crate::entity::{Account, AccountEvent, EventId};
use serde::{Deserialize, Serialize};
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
pub struct AccountId(i64);

impl Default for AccountId {
    fn default() -> Self {
        AccountId(crate::generate_id())
    }
}

impl From<AccountId> for EventId<AccountEvent, Account> {
    fn from(account_id: AccountId) -> Self {
        EventId::new(account_id.0)
    }
}
