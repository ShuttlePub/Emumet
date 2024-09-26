use crate::KernelError;
use error_stack::ResultExt;
use serde::{Deserialize, Deserializer, Serialize};
use std::marker::PhantomData;
use time::OffsetDateTime;
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Fromln, AsRefln)]
pub struct CreatedAt<T>(OffsetDateTime, PhantomData<T>);

impl<T> CreatedAt<T> {
    pub fn new(time: impl Into<OffsetDateTime>) -> Self {
        Self(time.into(), PhantomData)
    }

    pub fn now() -> error_stack::Result<Self, KernelError> {
        let now = OffsetDateTime::now_local()
            .change_context_lazy(|| KernelError::Internal)
            .attach_printable_lazy(|| "Failed to get current time")?;
        Ok(Self::new(now))
    }
}

// TODO: Remove
impl<T> Default for CreatedAt<T> {
    fn default() -> Self {
        Self(OffsetDateTime::now_utc(), PhantomData)
    }
}

impl<T> Serialize for CreatedAt<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for CreatedAt<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        <OffsetDateTime>::deserialize(deserializer).map(|time| Self::new(time))
    }
}
