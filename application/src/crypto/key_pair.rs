use error_stack::Result;
use kernel::KernelError;
use serde::{Deserialize, Serialize};

use super::algorithm::KeyAlgorithm;

/// Encrypted private key with metadata for decryption
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedPrivateKey {
    /// Base64-encoded ciphertext (encrypted PEM)
    pub ciphertext: String,
    /// Base64-encoded nonce (12 bytes for AES-GCM)
    pub nonce: String,
    /// Base64-encoded salt (16 bytes for Argon2id)
    pub salt: String,
    /// Algorithm used to generate the key pair
    pub algorithm: KeyAlgorithm,
}

/// Generated key pair with public key in PEM format and encrypted private key
pub struct GeneratedKeyPair {
    /// Public key in PEM format
    pub public_key_pem: String,
    /// Encrypted private key with metadata
    pub encrypted_private_key: EncryptedPrivateKey,
}

/// Trait for key pair generation
pub trait KeyPairGenerator: Send + Sync {
    /// Generate a new key pair, encrypting the private key with the given password
    fn generate(&self, password: &[u8]) -> Result<GeneratedKeyPair, KernelError>;
}
