use error_stack::{Report, Result};
use kernel::interfaces::crypto::{
    PreparedSigner, PreparedSignerFactory, RawKeyGenerator, RawKeyPair, SignatureVerifier, Signer,
    SigningAlgorithm,
};
use kernel::KernelError;
use rand::rngs::OsRng;
use rsa::sha2::Sha256;
use rsa::{
    pkcs1v15::{SigningKey, VerifyingKey},
    pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey, LineEnding},
    signature::{SignatureEncoding, Signer as RsaSigner, Verifier},
    RsaPrivateKey, RsaPublicKey,
};
use zeroize::Zeroizing;

/// RSA-2048 raw key pair generator (without encryption)
#[derive(Debug, Clone, Copy, Default)]
pub struct Rsa2048RawGenerator;

impl RawKeyGenerator for Rsa2048RawGenerator {
    fn generate_raw(&self) -> Result<RawKeyPair, KernelError> {
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

        Ok(RawKeyPair {
            public_key_pem,
            // Note: SecretDocument (private_key_pem source) already implements Zeroize on drop.
            // The copy to Vec<u8> is wrapped in Zeroizing for defense in depth.
            private_key_pem: Zeroizing::new(private_key_pem.as_bytes().to_vec()),
            algorithm: SigningAlgorithm::Rsa2048,
        })
    }

    fn algorithm(&self) -> SigningAlgorithm {
        SigningAlgorithm::Rsa2048
    }
}

/// RSA-2048 signer using PKCS#1 v1.5 + SHA-256
#[derive(Debug, Clone, Copy, Default)]
pub struct Rsa2048Signer;

impl Signer for Rsa2048Signer {
    fn sign(&self, data: &[u8], private_key_pem: &[u8]) -> Result<Vec<u8>, KernelError> {
        let pem_str = std::str::from_utf8(private_key_pem).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Invalid UTF-8 in private key PEM: {e}"))
        })?;

        let private_key = RsaPrivateKey::from_pkcs8_pem(pem_str).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to parse private key PEM: {e}"))
        })?;

        let signing_key = SigningKey::<Sha256>::new(private_key);
        let signature = signing_key.sign(data);

        Ok(signature.to_bytes().to_vec())
    }
}

/// Parses an RSA-2048 private key once into a signer that holds the parsed key.
#[derive(Debug, Clone, Copy, Default)]
pub struct Rsa2048PreparedSignerFactory;

impl PreparedSignerFactory for Rsa2048PreparedSignerFactory {
    fn prepare(
        &self,
        private_key_pem: &[u8],
        algorithm: SigningAlgorithm,
    ) -> Result<Box<dyn PreparedSigner>, KernelError> {
        if algorithm != SigningAlgorithm::Rsa2048 {
            return Err(Report::new(KernelError::Internal)
                .attach_printable(format!("Not an RSA-2048 key: {algorithm}")));
        }
        let pem_str = std::str::from_utf8(private_key_pem).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Invalid UTF-8 in private key PEM: {e}"))
        })?;
        let private_key = RsaPrivateKey::from_pkcs8_pem(pem_str).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to parse private key PEM: {e}"))
        })?;
        Ok(Box::new(Rsa2048PreparedSigner {
            signing_key: SigningKey::<Sha256>::new(private_key),
        }))
    }
}

/// RSA-2048 (PKCS#1 v1.5 + SHA-256) signer over an already parsed key. The `rsa` private key
/// zeroizes itself on drop; `Debug` prints nothing of it.
struct Rsa2048PreparedSigner {
    signing_key: SigningKey<Sha256>,
}

impl PreparedSigner for Rsa2048PreparedSigner {
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, KernelError> {
        Ok(self.signing_key.sign(data).to_bytes().to_vec())
    }

    fn algorithm(&self) -> SigningAlgorithm {
        SigningAlgorithm::Rsa2048
    }
}

/// RSA-2048 signature verifier using PKCS#1 v1.5 + SHA-256
#[derive(Debug, Clone, Copy, Default)]
pub struct Rsa2048Verifier;

impl SignatureVerifier for Rsa2048Verifier {
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

        let public_key = RsaPublicKey::from_public_key_pem(pem_str).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to parse public key PEM: {e}"))
        })?;

        let verifying_key = VerifyingKey::<Sha256>::new(public_key);

        let sig = rsa::pkcs1v15::Signature::try_from(signature).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Invalid signature format: {e}"))
        })?;

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
    fn test_rsa2048_raw_generation() {
        let generator = Rsa2048RawGenerator;

        let result = generator.generate_raw();
        assert!(result.is_ok());

        let key_pair = result.unwrap();
        assert!(key_pair.public_key_pem.contains("BEGIN PUBLIC KEY"));
        assert!(!key_pair.private_key_pem.is_empty());
        assert_eq!(key_pair.algorithm, SigningAlgorithm::Rsa2048);

        // Verify private key is valid PEM
        let private_key_str = String::from_utf8_lossy(&key_pair.private_key_pem);
        assert!(private_key_str.contains("BEGIN PRIVATE KEY"));
    }

    #[test]
    fn test_sign_and_verify() {
        let generator = Rsa2048RawGenerator;
        let key_pair = generator.generate_raw().unwrap();

        let signer = Rsa2048Signer;
        let verifier = Rsa2048Verifier;

        let data = b"Hello, ActivityPub!";
        let signature = signer.sign(data, &key_pair.private_key_pem).unwrap();

        let is_valid = verifier
            .verify(data, &signature, key_pair.public_key_pem.as_bytes())
            .unwrap();
        assert!(is_valid);
    }

    #[test]
    fn prepared_signer_signs_like_the_per_call_signer_and_rejects_other_algorithms() {
        let key_pair = Rsa2048RawGenerator.generate_raw().unwrap();
        let prepared = Rsa2048PreparedSignerFactory
            .prepare(&key_pair.private_key_pem, SigningAlgorithm::Rsa2048)
            .unwrap();
        let data = b"prepared signer";
        // PKCS#1 v1.5 is deterministic: both paths produce the same bytes.
        assert_eq!(
            prepared.sign(data).unwrap(),
            Rsa2048Signer.sign(data, &key_pair.private_key_pem).unwrap()
        );
        assert_eq!(prepared.algorithm(), SigningAlgorithm::Rsa2048);
        assert!(Rsa2048Verifier
            .verify(
                data,
                &prepared.sign(data).unwrap(),
                key_pair.public_key_pem.as_bytes()
            )
            .unwrap());
        assert!(Rsa2048PreparedSignerFactory
            .prepare(&key_pair.private_key_pem, SigningAlgorithm::Ed25519)
            .is_err());
        assert!(Rsa2048PreparedSignerFactory
            .prepare(b"not a pem", SigningAlgorithm::Rsa2048)
            .is_err());
    }

    #[test]
    fn test_verify_wrong_data_fails() {
        let generator = Rsa2048RawGenerator;
        let key_pair = generator.generate_raw().unwrap();

        let signer = Rsa2048Signer;
        let verifier = Rsa2048Verifier;

        let data = b"Original message";
        let signature = signer.sign(data, &key_pair.private_key_pem).unwrap();

        let tampered_data = b"Tampered message";
        let is_valid = verifier
            .verify(
                tampered_data,
                &signature,
                key_pair.public_key_pem.as_bytes(),
            )
            .unwrap();
        assert!(!is_valid);
    }
}
