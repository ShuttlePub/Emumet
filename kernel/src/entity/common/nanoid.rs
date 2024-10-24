use nanoid::nanoid;
use serde::{Deserialize, Deserializer, Serialize};
use std::marker::PhantomData;
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Fromln, AsRefln)]
pub struct Nanoid<T>(String, PhantomData<T>);

impl<T> Nanoid<T> {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into(), PhantomData)
    }
}

impl<T> Default for Nanoid<T> {
    fn default() -> Self {
        Self::new(nanoid!())
    }
}

impl<T> Serialize for Nanoid<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for Nanoid<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        <String>::deserialize(deserializer).map(Self::new)
    }
}
