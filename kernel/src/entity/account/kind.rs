use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    #[default]
    Personal,
    Organization,
}
