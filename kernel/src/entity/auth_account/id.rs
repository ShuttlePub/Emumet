use crate::entity::{AuthAccount, AuthAccountEvent, EventId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vodca::{AsRefln, Fromln, Newln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct AuthAccountId(Uuid);

impl Default for AuthAccountId {
    fn default() -> Self {
        AuthAccountId(Uuid::now_v7())
    }
}

impl From<AuthAccountId> for EventId<AuthAccountEvent, AuthAccount> {
    fn from(auth_account_id: AuthAccountId) -> Self {
        EventId::new(auth_account_id.0)
    }
}
