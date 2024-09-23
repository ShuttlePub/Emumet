use crate::entity::{EventId, RemoteAccount, RemoteAccountEvent};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vodca::{AsRefln, Fromln, Newln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct RemoteAccountId(Uuid);

impl From<RemoteAccountId> for EventId<RemoteAccountEvent, RemoteAccount> {
    fn from(id: RemoteAccountId) -> Self {
        EventId::new(id.0)
    }
}
