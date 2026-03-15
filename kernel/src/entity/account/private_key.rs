use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

#[derive(Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct AccountPrivateKey(String);

impl std::fmt::Debug for AccountPrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AccountPrivateKey")
            .field(&"[REDACTED]")
            .finish()
    }
}
