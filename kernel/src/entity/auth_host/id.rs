use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct AuthHostId(i64);

impl Default for AuthHostId {
    fn default() -> Self {
        AuthHostId(crate::generate_id())
    }
}
