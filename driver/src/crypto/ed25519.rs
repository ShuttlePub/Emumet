use ed25519_dalek::pkcs8::spki::{der::pem::LineEnding, EncodePublicKey};
use ed25519_dalek::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use ed25519_dalek::{Signature, VerifyingKey};
use error_stack::{Report, Result};
use kernel::interfaces::crypto::{
    RawKeyGenerator, RawKeyPair, SignatureVerifier, Signer, SigningAlgorithm,
};
use kernel::KernelError;
use rand::rngs::OsRng;
use zeroize::Zeroizing;

/// Ed25519 raw key pair generator (without encryption)
#[derive(Debug, Clone, Copy, Default)]
pub struct Ed25519RawGenerator;

impl RawKeyGenerator for Ed25519RawGenerator {
    fn generate_raw(&self) -> Result<RawKeyPair, KernelError> {
        let mut csprng = OsRng;
        let signing_key = ed25519_dalek::SigningKey::generate(&mut csprng);
        let verifying_key = VerifyingKey::from(&signing_key);

        let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to encode Ed25519 private key as PEM: {e}"))
        })?;

        let public_key_pem = verifying_key
            .to_public_key_pem(LineEnding::LF)
            .map_err(|e| {
                Report::new(KernelError::Internal)
                    .attach_printable(format!("Failed to encode Ed25519 public key as PEM: {e}"))
            })?;

        Ok(RawKeyPair {
            public_key_pem,
            private_key_pem: Zeroizing::new(private_key_pem.as_bytes().to_vec()),
            algorithm: SigningAlgorithm::Ed25519,
        })
    }

    fn algorithm(&self) -> SigningAlgorithm {
        SigningAlgorithm::Ed25519
    }
}

/// Ed25519 signer
#[derive(Debug, Clone, Copy, Default)]
pub struct Ed25519Signer;

impl Signer for Ed25519Signer {
    fn sign(&self, data: &[u8], private_key_pem: &[u8]) -> Result<Vec<u8>, KernelError> {
        let pem_str = std::str::from_utf8(private_key_pem).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Invalid UTF-8 in private key PEM: {e}"))
        })?;

        let signing_key = ed25519_dalek::SigningKey::from_pkcs8_pem(pem_str).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to parse Ed25519 private key PEM: {e}"))
        })?;

        use ed25519_dalek::Signer as DalekSigner;
        let signature: Signature = signing_key.sign(data);

        Ok(signature.to_bytes().to_vec())
    }
}

/// Ed25519 signature verifier
#[derive(Debug, Clone, Copy, Default)]
pub struct Ed25519Verifier;

impl SignatureVerifier for Ed25519Verifier {
    fn verify(
        &self,
        data: &[u8],
        signature: &[u8],
        public_key_pem: &[u8],
    ) -> Result<bool, KernelError> {
        let pem_str = std::str::from_utf8(public_key_pem).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Invalid UTF-8 in public key PEM: {e}"))
        })?;

        use ed25519_dalek::pkcs8::spki::DecodePublicKey;
        let verifying_key = VerifyingKey::from_public_key_pem(pem_str).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to parse Ed25519 public key PEM: {e}"))
        })?;

        let sig = Signature::from_slice(signature).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Invalid Ed25519 signature format: {e}"))
        })?;

        use ed25519_dalek::Verifier as DalekVerifier;
        match verifying_key.verify(data, &sig) {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ed25519_generate_test() {
        let generator = Ed25519RawGenerator;

        let result = generator.generate_raw();
        assert!(result.is_ok());

        let key_pair = result.unwrap();
        assert!(key_pair.public_key_pem.contains("BEGIN PUBLIC KEY"));
        assert!(!key_pair.private_key_pem.is_empty());
        assert_eq!(key_pair.algorithm, SigningAlgorithm::Ed25519);

        let private_key_str = String::from_utf8_lossy(&key_pair.private_key_pem);
        assert!(private_key_str.contains("BEGIN PRIVATE KEY"));
    }

    #[test]
    fn ed25519_sign_verify_roundtrip() {
        let generator = Ed25519RawGenerator;
        let key_pair = generator.generate_raw().unwrap();

        let signer = Ed25519Signer;
        let verifier = Ed25519Verifier;

        let data = b"hello world";
        let signature = signer.sign(data, &key_pair.private_key_pem).unwrap();

        let is_valid = verifier
            .verify(data, &signature, key_pair.public_key_pem.as_bytes())
            .unwrap();
        assert!(is_valid);
    }

    #[test]
    fn ed25519_tamper_detection() {
        let generator = Ed25519RawGenerator;
        let key_pair = generator.generate_raw().unwrap();

        let signer = Ed25519Signer;
        let verifier = Ed25519Verifier;

        let data = b"original message";
        let signature = signer.sign(data, &key_pair.private_key_pem).unwrap();

        let mut tampered_signature = signature.clone();
        tampered_signature[0] ^= 0xff;

        let is_valid = verifier
            .verify(
                data,
                &tampered_signature,
                key_pair.public_key_pem.as_bytes(),
            )
            .unwrap();
        assert!(!is_valid);
    }
}
