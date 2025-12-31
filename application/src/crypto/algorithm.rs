use serde::{Deserialize, Serialize};

/// Supported key algorithms for account key pairs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum KeyAlgorithm {
    #[default]
    Rsa2048,
    // Future: Ed25519, Rsa4096, etc.
}

impl std::fmt::Display for KeyAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rsa2048 => write!(f, "rsa2048"),
        }
    }
}
