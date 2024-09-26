use serde::{Deserialize, Deserializer, Serialize};
use std::marker::PhantomData;
use uuid::Uuid;
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Fromln, AsRefln)]
pub struct EventId<Event, Entity>(Uuid, PhantomData<Event>, PhantomData<Entity>);

impl<Ev, En> EventId<Ev, En> {
    pub fn new(id: Uuid) -> Self {
        Self(id, PhantomData, PhantomData)
    }

    pub fn raw_id(self) -> Uuid {
        self.0
    }
}

impl<Ev, En> Serialize for EventId<Ev, En> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de, Ev, En> Deserialize<'de> for EventId<Ev, En> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        <Uuid>::deserialize(deserializer).map(Self::new)
    }
}
