mod id;
mod url;

use serde::{Deserialize, Serialize};
use vodca::References;

pub use self::id::*;
pub use self::url::*;

#[derive(Debug, Clone, References, Serialize, Deserialize)]
pub struct RemoteAccount {
    id: RemoteAccountId,
    url: RemoteAccountUrl,
}

impl RemoteAccount {
    pub fn new(id: RemoteAccountId, url: RemoteAccountUrl) -> Self {
        Self { id, url }
    }
}
