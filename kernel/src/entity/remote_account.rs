mod url;

use serde::{Deserialize, Serialize};

pub use self::url::*;

use super::Id;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAccount {
    id: Id<RemoteAccount>,
    url: Url,
}

impl RemoteAccount {
    pub fn new(id: impl Into<i64>, url: impl Into<String>) -> Self {
        Self {
            id: Id::new(id),
            url: Url::new(url),
        }
    }

    pub fn id(&self) -> &Id<RemoteAccount> {
        &self.id
    }

    pub fn url(&self) -> &Url {
        &self.url
    }
}
