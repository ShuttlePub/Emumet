mod approved_at;
mod id;
mod target_id;

pub use self::{approved_at::*, id::*, target_id::*};

use crate::KernelError;
use error_stack::ResultExt;
use serde::{Deserialize, Serialize};
use vodca::References;

#[derive(Debug, Clone, Hash, Eq, PartialEq, References, Serialize, Deserialize)]
pub struct Follow {
    id: FollowId,
    source: FollowTargetId,
    destination: FollowTargetId,
    approved_at: Option<FollowApprovedAt>,
}

impl Follow {
    pub fn new(
        id: FollowId,
        source: FollowTargetId,
        destination: FollowTargetId,
        approved_at: Option<FollowApprovedAt>,
    ) -> error_stack::Result<Self, KernelError> {
        match (source, destination) {
            (source @ FollowTargetId::Remote(_), destination @ FollowTargetId::Remote(_)) => {
                Err(KernelError::Internal).attach_printable(format!(
                    "Cannot create remote to remote follow data. source: {:?}, destination: {:?}",
                    source, destination
                ))
            }
            (source, destination) => Ok(Self {
                id,
                source,
                destination,
                approved_at,
            }),
        }
    }
}
