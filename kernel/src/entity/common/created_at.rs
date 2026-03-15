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

    pub fn now() -> Self {
        let now = OffsetDateTime::now_utc();
        Self::new(now)
    }

    pub fn from_timestamp_ms(ms: u64) -> error_stack::Result<Self, KernelError> {
        let secs = (ms / 1000) as i64;
        let nanos = ((ms % 1000) * 1_000_000) as u32;
        let datetime = OffsetDateTime::from_unix_timestamp(secs)
            .change_context_lazy(|| KernelError::Internal)
            .attach_printable_lazy(|| format!("Invalid seconds: {secs}"))?
            .replace_nanosecond(nanos)
            .change_context_lazy(|| KernelError::Internal)
            .attach_printable_lazy(|| format!("Invalid nanos: {nanos}"))?;
        Ok(Self::new(datetime))
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
        <OffsetDateTime>::deserialize(deserializer).map(Self::new)
    }
}
