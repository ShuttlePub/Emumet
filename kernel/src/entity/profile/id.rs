use crate::entity::{EventId, Profile, ProfileEvent};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vodca::{AsRefln, Fromln, Newln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct ProfileId(Uuid);

impl From<ProfileId> for EventId<ProfileEvent, Profile> {
    fn from(profile_id: ProfileId) -> Self {
        EventId::new(profile_id.0)
    }
}
