use std::marker::PhantomData;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Id<T>(i64, PhantomData<T>);

impl<T> Id<T> {
    pub fn new(id: impl Into<i64>) -> Self {
        Self(id.into(), PhantomData)
    }
}

impl<T> From<Id<T>> for i64 {
    fn from(value: Id<T>) -> Self {
        value.0
    }
}

impl<T> AsRef<i64> for Id<T> {
    fn as_ref(&self) -> &i64 {
        &self.0
    }
}
