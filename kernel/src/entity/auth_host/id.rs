use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vodca::{AsRefln, Fromln, Newln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct AuthHostId(Uuid);

impl Default for AuthHostId {
    fn default() -> Self {
        AuthHostId(Uuid::now_v7())
    }
}
