mod id;
mod url;

pub use self::{id::*, url::*};
use serde::{Deserialize, Serialize};
use vodca::{Newln, References};

#[derive(Debug, Clone, PartialEq, Eq, Hash, References, Newln, Serialize, Deserialize)]
pub struct StellarHost {
    id: StellarHostId,
    url: StellarHostUrl,
}
