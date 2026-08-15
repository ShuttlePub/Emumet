use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct AuthAccountId(i64);

impl Default for AuthAccountId {
    fn default() -> Self {
        AuthAccountId(crate::generate_id())
    }
}
