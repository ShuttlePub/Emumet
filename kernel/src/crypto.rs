use crate::KernelError;
use error_stack::Result;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// Supported signing algorithms
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SigningAlgorithm {
    Rsa2048,
    Ed25519,
}

impl std::fmt::Display for SigningAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rsa2048 => write!(f, "rsa2048"),
            Self::Ed25519 => write!(f, "ed25519"),
        }
    }
}

impl Default for SigningAlgorithm {
    fn default() -> Self {
        Self::Rsa2048
    }
}

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
    pub algorithm: SigningAlgorithm,
}

/// Generated key pair with public key in PEM format and encrypted private key
pub struct GeneratedKeyPair {
    /// Public key in PEM format
    pub public_key_pem: String,
    /// Encrypted private key with metadata
    pub encrypted_private_key: EncryptedPrivateKey,
}

/// Raw key pair before encryption (private key is zeroized on drop)
pub struct RawKeyPair {
    /// Public key in PEM format
    pub public_key_pem: String,
    /// Private key in PEM format (zeroized on drop)
    pub private_key_pem: Zeroizing<Vec<u8>>,
    /// Algorithm used to generate the key pair
    pub algorithm: SigningAlgorithm,
}

/// Trait for providing master password
pub trait PasswordProvider: Send + Sync {
    fn get_password(&self) -> Result<Zeroizing<Vec<u8>>, KernelError>;
}

/// Trait for raw key pair generation (without encryption)
pub trait RawKeyGenerator: Send + Sync {
    /// Generate a new raw key pair (unencrypted)
    fn generate_raw(&self) -> Result<RawKeyPair, KernelError>;

    /// Returns the algorithm used by this generator
    fn algorithm(&self) -> SigningAlgorithm;
}

/// Trait for encrypting/decrypting private keys
pub trait KeyEncryptor: Send + Sync {
    fn encrypt(
        &self,
        private_key_pem: &[u8],
        password: &[u8],
        algorithm: SigningAlgorithm,
    ) -> Result<EncryptedPrivateKey, KernelError>;

    fn decrypt(
        &self,
        encrypted: &EncryptedPrivateKey,
        password: &[u8],
    ) -> Result<Vec<u8>, KernelError>;
}

/// Trait for signing data with a private key
pub trait Signer: Send + Sync {
    /// Sign data using PKCS#1 v1.5 + SHA-256 (for RSA) or Ed25519
    fn sign(&self, data: &[u8], private_key_pem: &[u8]) -> Result<Vec<u8>, KernelError>;
}

/// Trait for verifying signatures with a public key
pub trait SignatureVerifier: Send + Sync {
    /// Verify a signature using the public key
    fn verify(
        &self,
        data: &[u8],
        signature: &[u8],
        public_key_pem: &[u8],
    ) -> Result<bool, KernelError>;
}

// --- DI Traits ---

pub trait DependOnPasswordProvider: Send + Sync {
    type PasswordProvider: PasswordProvider;
    fn password_provider(&self) -> &Self::PasswordProvider;
}

pub trait DependOnRawKeyGenerator: Send + Sync {
    type RawKeyGenerator: RawKeyGenerator;
    fn raw_key_generator(&self) -> &Self::RawKeyGenerator;
}

pub trait DependOnKeyEncryptor: Send + Sync {
    type KeyEncryptor: KeyEncryptor;
    fn key_encryptor(&self) -> &Self::KeyEncryptor;
}

pub trait DependOnSigner: Send + Sync {
    type Signer: Signer;
    fn signer(&self) -> &Self::Signer;
}

pub trait DependOnSignatureVerifier: Send + Sync {
    type SignatureVerifier: SignatureVerifier;
    fn signature_verifier(&self) -> &Self::SignatureVerifier;
}
