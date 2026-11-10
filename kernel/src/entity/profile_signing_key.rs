use crate::crypto::{EncryptedPrivateKey, SigningAlgorithm};
use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{ProfileId, SigningKeyId};
use crate::KernelError;
use destructure::Destructure;
use std::future::Future;
use time::OffsetDateTime;
use vodca::{Newln, References};

/// Actor signing key owned by a Profile (ADR profile-actor-identity).
///
/// The owner is the Profile, never the Account: a Profile transfer does not change who owns the
/// key, the key id URI or the public key. At most one non-revoked key exists per Profile
/// (database unique index). P1 only provides the type and repository; production code does not
/// generate these keys until the P4 cutover.
#[derive(Debug, Clone, References, Newln, Destructure)]
pub struct ProfileSigningKey {
    id: SigningKeyId,
    profile_id: ProfileId,
    algorithm: SigningAlgorithm,
    encrypted_private_key: EncryptedPrivateKey,
    pub public_key_pem: String,
    pub key_id_uri: String,
    pub created_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
}

pub trait ProfileSigningKeyRepository: Sync + Send + 'static {
    type Connection: Connection;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &SigningKeyId,
    ) -> impl Future<Output = error_stack::Result<ProfileSigningKey, KernelError>> + Send;

    fn find_by_profile_id(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileSigningKey>, KernelError>> + Send;

    fn find_active_by_profile_id(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileSigningKey>, KernelError>> + Send;

    /// Fails with `KernelError::Concurrency` when the Profile already has an active key.
    fn create(
        &self,
        executor: &mut Self::Connection,
        signing_key: &ProfileSigningKey,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn revoke(
        &self,
        executor: &mut Self::Connection,
        id: &SigningKeyId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnProfileSigningKeyRepository: Sync + Send + DependOnDatabaseConnection {
    type ProfileSigningKeyRepository: ProfileSigningKeyRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_signing_key_repository(&self) -> &Self::ProfileSigningKeyRepository;
}
