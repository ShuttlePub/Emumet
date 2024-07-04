use crate::entity::CreatedAt;
use destructure::Destructure;
use vodca::References;

mod version;

pub use version::*;

#[derive(Debug, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, References, Destructure)]
pub struct EventEnvelope<Event, Entity> {
    event: Event,
    version: EventVersion<Entity>,
    created_at: CreatedAt<Entity>,
}

impl<Event, Entity> EventEnvelope<Event, Entity> {
    pub fn new(event: Event, version: EventVersion<Entity>, created_at: CreatedAt<Entity>) -> Self {
        Self {
            event,
            version,
            created_at,
        }
    }
}

#[derive(Debug, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, References, Destructure)]
pub struct CommandEnvelope<Event, Entity> {
    event: Event,
    version: Option<ExpectedEventVersion<Entity>>,
}

impl<Event, Entity> CommandEnvelope<Event, Entity> {
    pub(in crate::entity) fn new(
        event: Event,
        version: Option<ExpectedEventVersion<Entity>>,
    ) -> Self {
        Self { event, version }
    }
}
