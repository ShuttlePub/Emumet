use crate::entity::EventEnvelope;
use crate::KernelError;
use std::fmt::{Debug, Display};

pub trait EventApplier {
    type Event;
    const ENTITY_NAME: &'static str;

    fn apply(
        entity: &mut Option<Self>,
        event: EventEnvelope<Self::Event, Self>,
    ) -> error_stack::Result<(), KernelError>
    where
        Self: Sized;

    fn already_exists(entity: &Self) -> String
    where
        Self: Debug,
    {
        format!("{} already exists: {:?}", Self::ENTITY_NAME, entity)
    }

    fn not_exists(id: &impl Display) -> String {
        format!("{} not found: {}", Self::ENTITY_NAME, id)
    }
}
