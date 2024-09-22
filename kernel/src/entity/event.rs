use crate::entity::CreatedAt;
use destructure::Destructure;
use vodca::{Newln, References};

mod id;
mod version;

pub use {id::*, version::*};

#[derive(Debug, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, References, Newln, Destructure)]
pub struct EventEnvelope<Event, Entity> {
    id: EventId<Event, Entity>,
    event: Event,
    version: EventVersion<Entity>,
    created_at: CreatedAt<Entity>,
}

#[derive(Debug, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, References, Newln, Destructure)]
pub struct CommandEnvelope<Event, Entity> {
    id: EventId<Event, Entity>,
    event_name: String,
    event: Event,
    prev_version: Option<KnownEventVersion<Entity>>,
}
