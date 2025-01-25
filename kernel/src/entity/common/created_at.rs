use crate::KernelError;
use error_stack::{Report, ResultExt};
use serde::{Deserialize, Deserializer, Serialize};
use std::marker::PhantomData;
use time::OffsetDateTime;
use uuid::Timestamp;
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
}

impl<T> TryFrom<Timestamp> for CreatedAt<T> {
    type Error = Report<KernelError>;
    fn try_from(value: Timestamp) -> Result<Self, Self::Error> {
        let (seconds, nanos) = value.to_unix();
        let datetime = OffsetDateTime::from_unix_timestamp(seconds as i64)
            .change_context_lazy(|| KernelError::Internal)
            .attach_printable_lazy(|| format!("Invalid seconds: {seconds}"))?
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
