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

    /// Serializes concurrent key provisioning (transaction-scoped advisory lock, released on
    /// commit / rollback). Call first inside the provisioning transaction.
    fn lock_provisioning(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    /// All existing Profiles without an active key, oldest first (`(profile id, nanoid)`).
    /// Includes identities retained only in Created events (e.g. a deactivated owner Account),
    /// not just the current projection rows. id/nanoid are immutable across reactivation/transfer.
    /// Profiles that already have an active key are never returned, so provisioning can never
    /// regenerate or replace an existing identity key.
    fn find_profiles_without_active_key(
        &self,
        executor: &mut Self::Connection,
        limit: i64,
    ) -> impl Future<Output = error_stack::Result<Vec<(ProfileId, String)>, KernelError>> + Send;
}

pub trait DependOnProfileSigningKeyRepository: Sync + Send + DependOnDatabaseConnection {
    type ProfileSigningKeyRepository: ProfileSigningKeyRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_signing_key_repository(&self) -> &Self::ProfileSigningKeyRepository;
}
