use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use argon2::Argon2;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use error_stack::{Report, Result};
use kernel::interfaces::crypto::{EncryptedPrivateKey, KeyEncryptor, SigningAlgorithm};
use kernel::KernelError;
use rand::{rngs::OsRng, RngCore};
use zeroize::Zeroizing;

/// Argon2id parameters (OWASP recommended)
#[derive(Debug, Clone)]
pub struct Argon2Params {
    /// Memory cost in KiB (default: 64 MiB = 65536 KiB)
    pub memory_cost: u32,
    /// Number of iterations (default: 3)
    pub time_cost: u32,
    /// Degree of parallelism (default: 4)
    pub parallelism: u32,
}

impl Default for Argon2Params {
    fn default() -> Self {
        Self {
            memory_cost: 65536, // 64 MiB
            time_cost: 3,
            parallelism: 4,
        }
    }
}

/// Argon2id-based private key encryptor using AES-256-GCM
#[derive(Debug, Clone)]
pub struct Argon2Encryptor {
    params: Argon2Params,
}

impl Argon2Encryptor {
    pub fn new(params: Argon2Params) -> Self {
        Self { params }
    }
}

impl Default for Argon2Encryptor {
    fn default() -> Self {
        Self::new(Argon2Params::default())
    }
}

impl KeyEncryptor for Argon2Encryptor {
    fn encrypt(
        &self,
        private_key_pem: &[u8],
        password: &[u8],
        algorithm: SigningAlgorithm,
    ) -> Result<EncryptedPrivateKey, KernelError> {
        // Generate random salt (16 bytes)
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);

        // Derive key using Argon2id (32 bytes for AES-256)
        let argon2_params = argon2::Params::new(
            self.params.memory_cost,
            self.params.time_cost,
            self.params.parallelism,
            Some(32),
        )
        .map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Invalid Argon2 parameters: {e}"))
        })?;

        let argon2 = Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            argon2_params,
        );

        // Derived key is zeroized on drop
        let mut derived_key = Zeroizing::new([0u8; 32]);
        argon2
            .hash_password_into(password, &salt, &mut *derived_key)
            .map_err(|e| {
                Report::new(KernelError::Internal)
                    .attach_printable(format!("Argon2id key derivation failed: {e}"))
            })?;

        // Encrypt with AES-256-GCM
        let cipher = Aes256Gcm::new_from_slice(&*derived_key).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to create AES-GCM cipher: {e}"))
        })?;

        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher.encrypt(nonce, private_key_pem).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("AES-GCM encryption failed: {e}"))
        })?;

        Ok(EncryptedPrivateKey {
            ciphertext: BASE64.encode(&ciphertext),
            nonce: BASE64.encode(nonce_bytes),
            salt: BASE64.encode(salt),
            algorithm,
        })
    }

    fn decrypt(
        &self,
        encrypted: &EncryptedPrivateKey,
        password: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, KernelError> {
        // Decode Base64 fields (use generic error message to prevent information leakage)
        let salt = BASE64.decode(&encrypted.salt).map_err(|_| {
            Report::new(KernelError::Internal).attach_printable("Invalid encrypted data format")
        })?;

        let nonce_bytes = BASE64.decode(&encrypted.nonce).map_err(|_| {
            Report::new(KernelError::Internal).attach_printable("Invalid encrypted data format")
        })?;

        let ciphertext = BASE64.decode(&encrypted.ciphertext).map_err(|_| {
            Report::new(KernelError::Internal).attach_printable("Invalid encrypted data format")
        })?;

        // Derive key using Argon2id
        // Use generic error messages to prevent information leakage during decryption
        let argon2_params = argon2::Params::new(
            self.params.memory_cost,
            self.params.time_cost,
            self.params.parallelism,
            Some(32),
        )
        .map_err(|_| {
            Report::new(KernelError::Internal)
                .attach_printable("Decryption failed: invalid password or corrupted data")
        })?;

        let argon2 = Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            argon2_params,
        );

        // Derived key is zeroized on drop
        let mut derived_key = Zeroizing::new([0u8; 32]);
        argon2
            .hash_password_into(password, &salt, &mut *derived_key)
            .map_err(|_| {
                Report::new(KernelError::Internal)
                    .attach_printable("Decryption failed: invalid password or corrupted data")
            })?;

        // Decrypt with AES-256-GCM
        let cipher = Aes256Gcm::new_from_slice(&*derived_key).map_err(|_| {
            Report::new(KernelError::Internal)
                .attach_printable("Decryption failed: invalid password or corrupted data")
        })?;

        let nonce = Nonce::from_slice(&nonce_bytes);

        // Use generic error message to prevent timing attacks
        cipher
            .decrypt(nonce, ciphertext.as_ref())
            .map(Zeroizing::new)
            .map_err(|_| {
                Report::new(KernelError::Internal)
                    .attach_printable("Decryption failed: invalid password or corrupted data")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let original = b"-----BEGIN PRIVATE KEY-----\ntest data\n-----END PRIVATE KEY-----";
        let password = b"test-password-123";
        let encryptor = Argon2Encryptor::default();

        let encrypted = encryptor
            .encrypt(original, password, SigningAlgorithm::Rsa2048)
            .unwrap();

        assert!(!encrypted.ciphertext.is_empty());
        assert!(!encrypted.nonce.is_empty());
        assert!(!encrypted.salt.is_empty());
        assert_eq!(encrypted.algorithm, SigningAlgorithm::Rsa2048);

        let decrypted = encryptor.decrypt(&encrypted, password).unwrap();
        assert_eq!(decrypted.as_slice(), original);
    }

    #[test]
    fn test_wrong_password_fails() {
        let original = b"secret data";
        let password = b"correct-password";
        let wrong_password = b"wrong-password";
        let encryptor = Argon2Encryptor::default();

        let encrypted = encryptor
            .encrypt(original, password, SigningAlgorithm::Rsa2048)
            .unwrap();

        let result = encryptor.decrypt(&encrypted, wrong_password);
        assert!(result.is_err());
    }
}
