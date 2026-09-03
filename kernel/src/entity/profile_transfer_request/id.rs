use crate::entity::{EventId, ProfileTransferRequest, ProfileTransferRequestEvent};
use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct ProfileTransferRequestId(i64);

impl From<ProfileTransferRequestId>
    for EventId<ProfileTransferRequestEvent, ProfileTransferRequest>
{
    fn from(profile_transfer_request_id: ProfileTransferRequestId) -> Self {
        EventId::new(profile_transfer_request_id.0)
    }
}
