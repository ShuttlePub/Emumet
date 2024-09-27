use serde::{Deserialize, Deserializer, Serialize};
use std::marker::PhantomData;
use uuid::Uuid;
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Fromln, AsRefln)]
pub struct EventVersion<T>(Uuid, PhantomData<T>);

impl<T> EventVersion<T> {
    pub fn new(version: Uuid) -> Self {
        Self(version, PhantomData)
    }
}

impl<T> Serialize for EventVersion<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for EventVersion<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        <Uuid>::deserialize(deserializer).map(Self::new)
    }
}

#[derive(Debug, Clone, Ord, PartialOrd, PartialEq, Eq, Hash)]
pub enum KnownEventVersion<T> {
    /// There is no event stream
    Nothing,
    /// There is an event stream and the version is the past event version of the event stream
    Prev(EventVersion<T>),
}
