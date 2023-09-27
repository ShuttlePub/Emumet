use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IsBot(bool);

impl IsBot {
    pub fn new(is_bot: impl Into<bool>) -> Self {
        Self(is_bot.into())
    }
}

impl From<IsBot> for bool {
    fn from(is_bot: IsBot) -> Self {
        is_bot.0
    }
}

impl AsRef<bool> for IsBot {
    fn as_ref(&self) -> &bool {
        &self.0
    }
}
