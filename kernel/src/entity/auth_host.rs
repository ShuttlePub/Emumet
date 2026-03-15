mod id;
mod url;

pub use self::{id::*, url::*};
use destructure::Destructure;
use serde::{Deserialize, Serialize};
use vodca::{Newln, References};

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, References, Newln, Serialize, Deserialize, Destructure,
)]
pub struct AuthHost {
    id: AuthHostId,
    url: AuthHostUrl,
}
