mod id;
mod target_id;

pub use self::{id::*, target_id::*};

use crate::KernelError;
use error_stack::ResultExt;
use serde::{Deserialize, Serialize};
use vodca::References;

#[derive(Debug, Clone, Hash, Eq, PartialEq, References, Serialize, Deserialize)]
pub struct Mute {
    id: MuteId,
    source: MuteTargetId,
    destination: MuteTargetId,
}

impl Mute {
    pub fn new(
        id: MuteId,
        source: MuteTargetId,
        destination: MuteTargetId,
    ) -> error_stack::Result<Self, KernelError> {
        match (source, destination) {
            (source @ MuteTargetId::Remote(_), destination @ MuteTargetId::Remote(_)) => {
                Err(KernelError::Internal).attach_printable(format!(
                    "Cannot create remote to remote mute data. source: {:?}, destination: {:?}",
                    source, destination
                ))
            }
            (source, destination) => Ok(Self {
                id,
                source,
                destination,
            }),
        }
    }
}
