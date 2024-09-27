use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

/// Acct means webfinger url like: `username@domain`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Deserialize, Serialize)]
pub struct RemoteAccountAcct(String);
