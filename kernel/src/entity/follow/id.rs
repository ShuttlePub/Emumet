use crate::entity::{EventId, Follow, FollowEvent};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vodca::{AsRefln, Fromln, Newln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct FollowId(Uuid);

impl From<FollowId> for EventId<FollowEvent, Follow> {
    fn from(value: FollowId) -> Self {
        EventId::new(value.0)
    }
}
