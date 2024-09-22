use crate::entity::{EventId, Metadata, MetadataEvent};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vodca::{AsRefln, Fromln, Newln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct MetadataId(Uuid);

impl From<MetadataId> for EventId<MetadataEvent, Metadata> {
    fn from(metadata_id: MetadataId) -> Self {
        EventId::new(metadata_id.0)
    }
}
