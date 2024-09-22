use serde::{Deserialize, Deserializer, Serialize};
use std::marker::PhantomData;
use time::OffsetDateTime;
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, Hash, Eq, PartialEq, Fromln, AsRefln)]
pub struct DeletedAt<T>(OffsetDateTime, PhantomData<T>);

impl<T> DeletedAt<T> {
    pub fn new(time: impl Into<OffsetDateTime>) -> Self {
        Self(time.into(), PhantomData)
    }
}

impl<T> Default for DeletedAt<T> {
    fn default() -> Self {
        Self(OffsetDateTime::now_utc(), PhantomData)
    }
}

impl<T> Serialize for DeletedAt<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for DeletedAt<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        <OffsetDateTime>::deserialize(deserializer).map(|time| Self::new(time))
    }
}
