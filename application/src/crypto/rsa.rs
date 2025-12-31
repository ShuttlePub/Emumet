use error_stack::{Report, Result};
use kernel::KernelError;
use rand::rngs::OsRng;
use rsa::{
    pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding},
    RsaPrivateKey, RsaPublicKey,
};
use zeroize::Zeroizing;

use super::{
    algorithm::KeyAlgorithm,
    encryption::{encrypt_private_key, Argon2Params},
    key_pair::{GeneratedKeyPair, KeyPairGenerator},
};

/// RSA-2048 key pair generator
pub struct Rsa2048Generator {
    argon2_params: Argon2Params,
}

impl Rsa2048Generator {
    /// Create a new generator with default Argon2 parameters
    pub fn new() -> Self {
        Self {
            argon2_params: Argon2Params::default(),
        }
    }

    /// Create a generator with custom Argon2 parameters
    pub fn with_argon2_params(params: Argon2Params) -> Self {
        Self {
            argon2_params: params,
        }
    }
}

impl Default for Rsa2048Generator {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyPairGenerator for Rsa2048Generator {
    fn generate(&self, password: &[u8]) -> Result<GeneratedKeyPair, KernelError> {
        // Generate RSA-2048 key pair
        let private_key = RsaPrivateKey::new(&mut OsRng, 2048).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to generate RSA-2048 key: {e}"))
        })?;

        let public_key = RsaPublicKey::from(&private_key);

        // Convert to PEM format
        // Note: SecretDocument already implements Zeroize on drop
        let private_key_pem = private_key.to_pkcs8_pem(LineEnding::LF).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to encode private key as PEM: {e}"))
        })?;

        let public_key_pem = public_key.to_public_key_pem(LineEnding::LF).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to encode public key as PEM: {e}"))
        })?;

        // Copy private key bytes with zeroize protection
        let private_key_bytes = Zeroizing::new(private_key_pem.as_bytes().to_vec());

        // Encrypt private key
        let encrypted_private_key = encrypt_private_key(
            &private_key_bytes,
            password,
            KeyAlgorithm::Rsa2048,
            &self.argon2_params,
        )?;

        Ok(GeneratedKeyPair {
            public_key_pem,
            encrypted_private_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::encryption::decrypt_private_key;

    #[test]
    fn test_rsa2048_generation() {
        let generator = Rsa2048Generator::new();
        let password = b"test-password-123";

        let result = generator.generate(password);
        assert!(result.is_ok());

        let key_pair = result.unwrap();
        assert!(key_pair.public_key_pem.contains("BEGIN PUBLIC KEY"));
        assert!(!key_pair.encrypted_private_key.ciphertext.is_empty());
        assert_eq!(
            key_pair.encrypted_private_key.algorithm,
            KeyAlgorithm::Rsa2048
        );
    }

    #[test]
    fn test_decrypt_generated_key() {
        let generator = Rsa2048Generator::new();
        let password = b"test-password-456";

        let key_pair = generator.generate(password).unwrap();

        let decrypted = decrypt_private_key(
            &key_pair.encrypted_private_key,
            password,
            &Argon2Params::default(),
        )
        .unwrap();

        let decrypted_str = String::from_utf8_lossy(&decrypted);
        assert!(decrypted_str.contains("BEGIN PRIVATE KEY"));
    }
}
