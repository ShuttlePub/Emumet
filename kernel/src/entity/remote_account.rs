mod url;

pub use self::url::*;

use super::Id;

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
