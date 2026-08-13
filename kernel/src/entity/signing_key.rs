use crate::crypto::{EncryptedPrivateKey, SigningAlgorithm};
use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::AccountId;
use crate::KernelError;
use destructure::Destructure;
use serde::{Deserialize, Serialize};
use std::future::Future;
use time::OffsetDateTime;
use vodca::{AsRefln, Fromln, Newln, References};
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    Fromln,
    AsRefln,
    Newln,
    Serialize,
    Deserialize,
)]
pub struct SigningKeyId(i64);

impl Default for SigningKeyId {
    fn default() -> Self {
        SigningKeyId(crate::generate_id())
    }
}

impl std::fmt::Display for SigningKeyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, References, Newln, Destructure)]
pub struct SigningKey {
    id: SigningKeyId,
    account_id: AccountId,
    algorithm: SigningAlgorithm,
    encrypted_private_key: EncryptedPrivateKey,
    pub public_key_pem: String,
    pub key_id_uri: String,
    pub created_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
}

pub trait SigningKeyRepository: Sync + Send + 'static {
    type Connection: Connection;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &SigningKeyId,
    ) -> impl Future<Output = error_stack::Result<SigningKey, KernelError>> + Send;

    fn find_by_account_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<SigningKey>, KernelError>> + Send;

    fn find_active_by_account_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<SigningKey>, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Connection,
        signing_key: &SigningKey,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn revoke(
        &self,
        executor: &mut Self::Connection,
        id: &SigningKeyId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnSigningKeyRepository: Sync + Send + DependOnDatabaseConnection {
    type SigningKeyRepository: SigningKeyRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn signing_key_repository(&self) -> &Self::SigningKeyRepository;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signing_key_id_default() {
        crate::id::ensure_generator_initialized();
        let id = SigningKeyId::default();
        assert!(id.0 > 0);
    }

    #[test]
    fn test_signing_key_id_display() {
        let id = SigningKeyId::new(42);
        assert_eq!(id.to_string(), "42");
    }

    #[test]
    fn test_signing_key_construction() {
        crate::id::ensure_generator_initialized();
        let key = SigningKey::new(
            SigningKeyId::default(),
            AccountId::default(),
            SigningAlgorithm::default(),
            EncryptedPrivateKey {
                ciphertext: "test".to_string(),
                nonce: "test".to_string(),
                salt: "test".to_string(),
                algorithm: SigningAlgorithm::default(),
            },
            "public_key_pem".to_string(),
            "https://example.com/keys/1".to_string(),
            OffsetDateTime::now_utc(),
            None,
        );
        assert!(key.revoked_at.is_none());
    }
}
