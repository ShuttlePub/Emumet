use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Serialize, Deserialize)]
pub struct AccountIsBot(bool);

impl AccountIsBot {
    pub fn new(is_bot: impl Into<bool>) -> Self {
        Self(is_bot.into())
    }
}
