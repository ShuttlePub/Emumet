mod id;
mod target_id;

pub use self::{id::*, target_id::*};

use crate::KernelError;
use error_stack::ResultExt;
use serde::{Deserialize, Serialize};
use vodca::References;

#[derive(Debug, Clone, Hash, Eq, PartialEq, References, Serialize, Deserialize)]
pub struct Block {
    id: BlockId,
    source: BlockTargetId,
    destination: BlockTargetId,
}

impl Block {
    pub fn new(
        id: BlockId,
        source: BlockTargetId,
        destination: BlockTargetId,
    ) -> error_stack::Result<Self, KernelError> {
        match (source, destination) {
            (source @ BlockTargetId::Remote(_), destination @ BlockTargetId::Remote(_)) => {
                Err(KernelError::Internal).attach_printable(format!(
                    "Cannot create remote to remote block data. source: {:?}, destination: {:?}",
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
