use std::marker::PhantomData;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, Hash, Fromln, AsRefln, Serialize, Deserialize)]
pub struct CreatedAt<T>(OffsetDateTime, PhantomData<T>);

impl<T> CreatedAt<T> {
    pub fn new(time: impl Into<OffsetDateTime>) -> Self {
        Self(time.into(), PhantomData)
    }
}

impl Default for CreatedAt<()> {
    fn default() -> Self {
        Self(OffsetDateTime::now_utc(), PhantomData)
    }
}
