mod acct;
mod id;
mod url;

pub use self::acct::*;
pub use self::id::*;
pub use self::url::*;
use crate::entity::image::ImageId;
use serde::{Deserialize, Serialize};
use vodca::{Newln, References};

#[derive(Debug, Clone, Eq, PartialEq, References, Newln, Serialize, Deserialize)]
pub struct RemoteAccount {
    id: RemoteAccountId,
    acct: RemoteAccountAcct,
    url: RemoteAccountUrl,
    icon_id: Option<ImageId>,
}
