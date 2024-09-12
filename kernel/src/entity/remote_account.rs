mod acct;
mod id;
mod url;

use crate::entity::image::ImageId;
use serde::{Deserialize, Serialize};
use vodca::References;

pub use self::acct::*;
pub use self::id::*;
pub use self::url::*;

#[derive(Debug, Clone, References, Serialize, Deserialize)]
pub struct RemoteAccount {
    id: RemoteAccountId,
    acct: RemoteAccountAcct,
    url: RemoteAccountUrl,
    icon_id: Option<ImageId>,
}

impl RemoteAccount {
    pub fn new(
        id: RemoteAccountId,
        acct: RemoteAccountAcct,
        url: RemoteAccountUrl,
        icon_id: Option<ImageId>,
    ) -> Self {
        Self {
            id,
            acct,
            url,
            icon_id,
        }
    }
}
