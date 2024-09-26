mod acct;
mod id;
mod url;

pub use self::acct::*;
pub use self::id::*;
pub use self::url::*;
use crate::entity::image::ImageId;
use crate::entity::{CommandEnvelope, EventId, KnownEventVersion};
use serde::{Deserialize, Serialize};
use vodca::{Nameln, Newln, References};

#[derive(Debug, Clone, Eq, PartialEq, References, Newln, Serialize, Deserialize)]
pub struct RemoteAccount {
    id: RemoteAccountId,
    acct: RemoteAccountAcct,
    url: RemoteAccountUrl,
    icon_id: Option<ImageId>,
}

#[derive(Debug, Clone, Eq, PartialEq, Nameln, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "snake_case")]
#[vodca(prefix = "remote_account", snake_case)]
pub enum RemoteAccountEvent {
    Created {
        acct: RemoteAccountAcct,
        url: RemoteAccountUrl,
        icon_id: Option<ImageId>,
    },
    Updated {
        icon_id: Option<ImageId>,
    },
    Deleted,
}

impl RemoteAccount {
    pub fn create(
        id: RemoteAccountId,
        acct: RemoteAccountAcct,
        url: RemoteAccountUrl,
        icon_id: Option<ImageId>,
    ) -> CommandEnvelope<RemoteAccountEvent, RemoteAccount> {
        let event = RemoteAccountEvent::Created { acct, url, icon_id };
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(KnownEventVersion::Nothing),
        )
    }

    pub fn update_icon_id(
        id: RemoteAccountId,
        icon_id: Option<ImageId>,
    ) -> CommandEnvelope<RemoteAccountEvent, RemoteAccount> {
        let event = RemoteAccountEvent::Updated { icon_id };
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }

    pub fn delete(id: RemoteAccountId) -> CommandEnvelope<RemoteAccountEvent, RemoteAccount> {
        let event = RemoteAccountEvent::Deleted;
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }
}
