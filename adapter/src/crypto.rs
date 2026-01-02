use error_stack::Result;
use kernel::interfaces::crypto::{
    DependOnKeyEncryptor, DependOnRawKeyGenerator, GeneratedKeyPair, KeyEncryptor, RawKeyGenerator,
    SigningAlgorithm,
};
use kernel::KernelError;

/// Trait for signing key pair generation (composed from RawKeyGenerator + KeyEncryptor)
///
/// This trait is automatically implemented for any type that implements both
/// [`DependOnRawKeyGenerator`] and [`DependOnKeyEncryptor`] via blanket implementation.
///
/// # Architecture
///
/// ```text
/// Application (uses SigningKeyGenerator)
///      ↓
/// Adapter (composes RawKeyGenerator + KeyEncryptor)
///      ↓
/// Kernel (defines traits)
///      ↑
/// Driver (implements concrete crypto)
/// ```
///
/// # Example
///
/// ```ignore
/// // If Handler implements DependOnRawKeyGenerator + DependOnKeyEncryptor,
/// // SigningKeyGenerator is automatically implemented.
/// let key_pair = handler.signing_key_generator().generate(password)?;
/// ```
pub trait SigningKeyGenerator: Send + Sync {
    /// Generate a new key pair, encrypting the private key with the given password
    fn generate(&self, password: &[u8]) -> Result<GeneratedKeyPair, KernelError>;

    /// Returns the algorithm used by this generator
    fn algorithm(&self) -> SigningAlgorithm;
}

pub trait DependOnSigningKeyGenerator: Send + Sync {
    type SigningKeyGenerator: SigningKeyGenerator;
    fn signing_key_generator(&self) -> &Self::SigningKeyGenerator;
}

// Blanket implementation: any type with RawKeyGenerator + KeyEncryptor can generate signing keys
impl<T> SigningKeyGenerator for T
where
    T: DependOnRawKeyGenerator + DependOnKeyEncryptor + Send + Sync,
{
    fn generate(&self, password: &[u8]) -> Result<GeneratedKeyPair, KernelError> {
        let raw = self.raw_key_generator().generate_raw()?;
        let encrypted =
            self.key_encryptor()
                .encrypt(&raw.private_key_pem, password, raw.algorithm)?;
        Ok(GeneratedKeyPair {
            public_key_pem: raw.public_key_pem,
            encrypted_private_key: encrypted,
        })
    }

    fn algorithm(&self) -> SigningAlgorithm {
        self.raw_key_generator().algorithm()
    }
}

// Blanket implementation: any type that can generate signing keys provides DependOnSigningKeyGenerator
impl<T> DependOnSigningKeyGenerator for T
where
    T: DependOnRawKeyGenerator + DependOnKeyEncryptor + Send + Sync,
{
    type SigningKeyGenerator = Self;
    fn signing_key_generator(&self) -> &Self::SigningKeyGenerator {
        self
    }
}
