use std::marker::PhantomData;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub struct CreatedAt<T>(OffsetDateTime, PhantomData<T>);

impl<T> CreatedAt<T> {
    pub fn new(time: impl Into<OffsetDateTime>) -> Self {
        Self(time.into(), PhantomData)
    }
}

impl<T> From<CreatedAt<T>> for OffsetDateTime {
    fn from(value: CreatedAt<T>) -> Self {
        value.0
    }
}

impl<T> AsRef<OffsetDateTime> for CreatedAt<T> {
    fn as_ref(&self) -> &OffsetDateTime {
        &self.0
    }
}

impl Default for CreatedAt<()> {
    fn default() -> Self {
        Self(OffsetDateTime::now_utc(), PhantomData)
    }
}
