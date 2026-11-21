//! The Emumet instance actor identity and its signing key (P1 post-relay, K1).
//!
//! * [`EnsureInstanceIdentityUseCase`] runs once at start-up: it creates the identity and its first
//!   key **only when neither exists**, serialized by an advisory lock and backed by database
//!   unique constraints so that concurrent starts produce exactly one identity and one active key.
//!   Every other state — a different configured origin, a missing / extra / damaged key, a key
//!   that cannot be decrypted — is an error that stops the start-up. Nothing is ever repaired by
//!   generating another key (rotation and revocation are K2 and have no entry point here).
//! * [`UnlockInstanceSigningKeyUseCase`] decrypts the active key into an in-process, bounded cache
//!   ([`InstanceKeyCache`]); the delivery worker (S1) signs with it without running the KDF per
//!   request.
//! * [`GetInstanceActorUseCase`] builds the public actor document. It exposes public keys only.
//!
//! Every URL is derived from the configured public origin, never from a request `Host`.

use error_stack::Report;
use kernel::activitypub::InstanceActor;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::crypto::{
    DependOnKeyEncryptor, DependOnPasswordProvider, DependOnPreparedSignerFactory,
    DependOnSignatureVerifier, DependOnSigningKeyGenerator, KeyEncryptor, PasswordProvider,
    PreparedSigner, PreparedSignerFactory, SignatureVerifier, SigningAlgorithm,
    SigningKeyGenerator,
};
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::repository::{
    DependOnInstanceIdentityRepository, InstanceIdentityRepository,
};
use kernel::prelude::entity::{
    InstanceIdentity, InstanceOrigin, InstanceSigningKey, InstanceSigningKeyId,
};
use kernel::KernelError;
use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Why the stored instance identity was refused. Attached to the error so that callers and tests
/// can tell the cases apart; none of them is repaired automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceIdentityProblem {
    /// The stored public origin differs from the configured `PUBLIC_BASE_URL`.
    OriginMismatch,
    /// Keys exist without an identity (impossible through this code; a damaged database).
    KeysWithoutIdentity,
    /// An identity exists but has no active (signing) key: every key is retired or revoked.
    NoActiveKey,
    /// More than one active key (the database unique index forbids it; a damaged database).
    MultipleActiveKeys,
    /// A stored key id URI differs from `{origin}/ap/instance#key-{id}`.
    KeyIdUriMismatch,
    /// The key uses an algorithm other than the RSA-2048 the post-relay contract requires.
    UnsupportedAlgorithm,
    /// The private key cannot be decrypted with the configured master key password.
    UndecryptableKey,
    /// The decrypted private key is not a usable key (cannot be parsed for signing).
    UnparsableKey,
    /// The private key does not belong to the stored public key.
    KeyPairMismatch,
}

impl Display for InstanceIdentityProblem {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::OriginMismatch => {
                "the stored instance public origin differs from the configured PUBLIC_BASE_URL"
            }
            Self::KeysWithoutIdentity => "instance signing keys exist without an instance identity",
            Self::NoActiveKey => "the instance identity has no active signing key",
            Self::MultipleActiveKeys => "the instance identity has more than one active key",
            Self::KeyIdUriMismatch => "a stored instance key id differs from the one derived from the origin",
            Self::UnsupportedAlgorithm => "the instance signing key does not use RSA-2048",
            Self::UndecryptableKey => {
                "the instance private key cannot be decrypted with the configured master key password"
            }
            Self::UnparsableKey => "the decrypted instance private key cannot be parsed for signing",
            Self::KeyPairMismatch => "the instance private key does not match the stored public key",
        })
    }
}

fn refuse(problem: InstanceIdentityProblem) -> Report<KernelError> {
    Report::new(KernelError::Internal).attach_printable(problem)
}

/// The active key of a stored identity, after checking everything that can be checked without the
/// master key password. Pure: no key is ever generated here.
pub fn select_active_key<'a>(
    origin: &InstanceOrigin,
    identity: &InstanceIdentity,
    keys: &'a [InstanceSigningKey],
) -> error_stack::Result<&'a InstanceSigningKey, KernelError> {
    if identity.public_origin() != origin {
        return Err(refuse(InstanceIdentityProblem::OriginMismatch));
    }
    if keys
        .iter()
        .any(|key| key.key_id_uri != origin.key_id_uri(key.id()))
    {
        return Err(refuse(InstanceIdentityProblem::KeyIdUriMismatch));
    }
    let mut active = keys.iter().filter(|key| key.is_active());
    let key = active
        .next()
        .ok_or_else(|| refuse(InstanceIdentityProblem::NoActiveKey))?;
    if active.next().is_some() {
        return Err(refuse(InstanceIdentityProblem::MultipleActiveKeys));
    }
    if *key.algorithm() != SigningAlgorithm::Rsa2048 {
        return Err(refuse(InstanceIdentityProblem::UnsupportedAlgorithm));
    }
    Ok(key)
}

// ---------------------------------------------------------------------------
// In-process cache of the unlocked key (plan 7.2)
// ---------------------------------------------------------------------------

/// Default cap of the cache: the active key and, during a later rotation, the previous one.
pub const INSTANCE_KEY_CACHE_MAX_ENTRIES: usize = 2;
/// Default lifetime of a cached entry.
pub const INSTANCE_KEY_CACHE_TTL: Duration = Duration::from_secs(15 * 60);

/// The unlocked instance signing key: the private key is decrypted **and parsed once** into a
/// [`PreparedSigner`], so signing with it neither runs the KDF nor parses the PEM again. The PEM
/// is not kept; the parsed key lives in memory only (it zeroizes itself on drop) and `Debug`
/// never prints it.
///
/// This is the handle boundary of K2: a revoked or retired key is replaced by loading another
/// handle under a different key id / revision, and nothing else holds key material.
pub struct UnlockedInstanceSigningKey {
    key_id: InstanceSigningKeyId,
    revision: i64,
    key_id_uri: String,
    algorithm: SigningAlgorithm,
    signer: Box<dyn PreparedSigner>,
}

impl UnlockedInstanceSigningKey {
    pub fn key_id(&self) -> &InstanceSigningKeyId {
        &self.key_id
    }

    pub fn revision(&self) -> i64 {
        self.revision
    }

    /// The `keyId` of the signatures made with this key.
    pub fn key_id_uri(&self) -> &str {
        &self.key_id_uri
    }

    pub fn algorithm(&self) -> SigningAlgorithm {
        self.algorithm
    }

    /// Signs with the parsed key (the S1 sender signs every attempt through this). No KDF, no PEM
    /// parse.
    pub fn sign(&self, data: &[u8]) -> error_stack::Result<Vec<u8>, KernelError> {
        self.signer.sign(data)
    }
}

impl Debug for UnlockedInstanceSigningKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnlockedInstanceSigningKey")
            .field("key_id_uri", &self.key_id_uri)
            .field("revision", &self.revision)
            .field("signer", &"[REDACTED]")
            .finish()
    }
}

struct CacheEntry {
    key_id: InstanceSigningKeyId,
    revision: i64,
    loaded_at: Instant,
    key: Arc<UnlockedInstanceSigningKey>,
}

/// Bounded in-process cache of unlocked instance keys.
///
/// * at most `max_entries` entries, keyed by key id **and** revision, so a changed key state is a
///   different entry (the oldest entry is evicted when the cap is exceeded);
/// * an entry lives at most `ttl`; afterwards the key is unlocked again from the database and a
///   failure then is an error, an expired entry is never used;
/// * a miss is single-flight: the cache lock is held while loading, so concurrent callers wait for
///   one load (one KDF run) and see the entry it produced.
///
/// The TTL bounds how long a key revoked in the database can still be used by a process that has
/// it cached. It is not an emergency revocation mechanism: revocation, with its notification and
/// invalidation, is K2 (the revision in the cache key is the hook for it).
pub struct InstanceKeyCache {
    max_entries: usize,
    ttl: Duration,
    entries: tokio::sync::Mutex<Vec<CacheEntry>>,
}

impl Default for InstanceKeyCache {
    fn default() -> Self {
        Self::new(INSTANCE_KEY_CACHE_MAX_ENTRIES, INSTANCE_KEY_CACHE_TTL)
    }
}

impl InstanceKeyCache {
    pub fn new(max_entries: usize, ttl: Duration) -> Self {
        Self {
            max_entries: max_entries.max(1),
            ttl,
            entries: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    pub async fn get_or_try_load<F, Fut>(
        &self,
        key_id: &InstanceSigningKeyId,
        revision: i64,
        load: F,
    ) -> error_stack::Result<Arc<UnlockedInstanceSigningKey>, KernelError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = error_stack::Result<UnlockedInstanceSigningKey, KernelError>>,
    {
        self.get_or_try_load_at(Instant::now(), key_id, revision, load)
            .await
    }

    pub(crate) async fn get_or_try_load_at<F, Fut>(
        &self,
        now: Instant,
        key_id: &InstanceSigningKeyId,
        revision: i64,
        load: F,
    ) -> error_stack::Result<Arc<UnlockedInstanceSigningKey>, KernelError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = error_stack::Result<UnlockedInstanceSigningKey, KernelError>>,
    {
        let mut entries = self.entries.lock().await;
        let ttl = self.ttl;
        entries.retain(|entry| now.saturating_duration_since(entry.loaded_at) < ttl);
        if let Some(entry) = entries
            .iter()
            .find(|entry| &entry.key_id == key_id && entry.revision == revision)
        {
            return Ok(entry.key.clone());
        }
        let key = Arc::new(load().await?);
        entries.push(CacheEntry {
            key_id: key_id.clone(),
            revision,
            loaded_at: now,
            key: key.clone(),
        });
        while entries.len() > self.max_entries {
            let oldest = entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.loaded_at)
                .map(|(index, _)| index)
                .expect("a non-empty cache has an oldest entry");
            entries.remove(oldest);
        }
        Ok(key)
    }

    #[cfg(test)]
    pub(crate) async fn len(&self) -> usize {
        self.entries.lock().await.len()
    }
}

pub trait DependOnInstanceKeyCache: Sync + Send {
    fn instance_key_cache(&self) -> &InstanceKeyCache;
}

// ---------------------------------------------------------------------------
// Unlock
// ---------------------------------------------------------------------------

const SELF_CHECK_PAYLOAD: &[u8] = b"emumet instance key self check";

pub trait UnlockInstanceSigningKeyUseCase:
    'static
    + Sync
    + Send
    + DependOnInstanceIdentityRepository
    + DependOnKeyEncryptor
    + DependOnPreparedSignerFactory
    + DependOnSignatureVerifier
    + DependOnPasswordProvider
    + DependOnPublicBaseUrl
    + DependOnInstanceKeyCache
{
    /// The unlocked active key. The KDF runs only on a cache miss (first use or after the TTL);
    /// a decryption failure is an error and never triggers a new key.
    fn unlock_instance_signing_key(
        &self,
    ) -> impl Future<Output = error_stack::Result<Arc<UnlockedInstanceSigningKey>, KernelError>> + Send
    {
        async move {
            let origin = InstanceOrigin::parse(self.public_base_url().as_str())?;
            let mut executor = self.database_connection().connection().await?;
            let repository = self.instance_identity_repository();
            let identity = repository
                .find_identity(&mut executor)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::Internal)
                        .attach_printable("the instance identity has not been provisioned")
                })?;
            let keys = repository.find_signing_keys(&mut executor).await?;
            drop(executor);
            let key = select_active_key(&origin, &identity, &keys)?;
            self.instance_key_cache()
                .get_or_try_load(key.id(), *key.revision(), || async {
                    let password = self.password_provider().get_password()?;
                    let private_key_pem = self
                        .key_encryptor()
                        .decrypt(key.encrypted_private_key(), &password)
                        .map_err(|e| e.attach_printable(InstanceIdentityProblem::UndecryptableKey))
                        .map_err(|e| e.change_context(KernelError::Internal))?;
                    // Parsed once here; the PEM is dropped (and zeroized) at the end of this block.
                    let signer = self
                        .prepared_signer_factory()
                        .prepare(&private_key_pem, *key.algorithm())
                        .map_err(|e| e.attach_printable(InstanceIdentityProblem::UnparsableKey))?;
                    drop(private_key_pem);
                    let signature = signer.sign(SELF_CHECK_PAYLOAD).map_err(|e| {
                        e.attach_printable(InstanceIdentityProblem::KeyPairMismatch)
                    })?;
                    let matches = self
                        .signature_verifier()
                        .verify(
                            SELF_CHECK_PAYLOAD,
                            &signature,
                            key.public_key_pem.as_bytes(),
                        )
                        .unwrap_or(false);
                    if !matches {
                        return Err(refuse(InstanceIdentityProblem::KeyPairMismatch));
                    }
                    Ok(UnlockedInstanceSigningKey {
                        key_id: key.id().clone(),
                        revision: *key.revision(),
                        key_id_uri: key.key_id_uri.clone(),
                        algorithm: *key.algorithm(),
                        signer,
                    })
                })
                .await
        }
    }
}

impl<T> UnlockInstanceSigningKeyUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnInstanceIdentityRepository
        + DependOnKeyEncryptor
        + DependOnPreparedSignerFactory
        + DependOnSignatureVerifier
        + DependOnPasswordProvider
        + DependOnPublicBaseUrl
        + DependOnInstanceKeyCache
{
}

// ---------------------------------------------------------------------------
// Provisioning
// ---------------------------------------------------------------------------

/// What [`EnsureInstanceIdentityUseCase::ensure_instance_identity`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceIdentityReport {
    /// `true` when this call created the identity and its key (first start).
    pub created: bool,
    /// The `keyId` of the active key.
    pub key_id_uri: String,
}

pub trait EnsureInstanceIdentityUseCase:
    'static
    + Clone
    + Sync
    + Send
    + DependOnTransactionManager
    + DependOnInstanceIdentityRepository
    + DependOnSigningKeyGenerator
    + DependOnPasswordProvider
    + DependOnPublicBaseUrl
    + UnlockInstanceSigningKeyUseCase
{
    /// Creates the identity and its first key if (and only if) neither exists, then proves that the
    /// stored state is usable: the origin matches, exactly one active key, its private part is
    /// decryptable and belongs to the public key. The unlocked key is left in the cache.
    ///
    /// Any inconsistency is an error. In particular an undecryptable or damaged key is **not** a
    /// missing key: no new key is generated and the existing rows are left untouched.
    fn ensure_instance_identity(
        &self,
    ) -> impl Future<Output = error_stack::Result<InstanceIdentityReport, KernelError>> + Send + '_
    {
        async move {
            let origin = InstanceOrigin::parse(self.public_base_url().as_str())?;
            let deps = self.clone();
            let created = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let repository = deps.instance_identity_repository();
                        repository.lock_provisioning(executor).await?;
                        let identity = repository.find_identity(executor).await?;
                        let keys = repository.find_signing_keys(executor).await?;
                        match identity {
                            Some(identity) => {
                                select_active_key(&origin, &identity, &keys)?;
                                Ok(false)
                            }
                            None if !keys.is_empty() => {
                                Err(refuse(InstanceIdentityProblem::KeysWithoutIdentity))
                            }
                            None => {
                                let password = deps.password_provider().get_password()?;
                                let key_pair = deps.signing_key_generator().generate(&password)?;
                                let now = time::OffsetDateTime::now_utc();
                                let key_id = InstanceSigningKeyId::generate();
                                let signing_key = InstanceSigningKey::new(
                                    key_id.clone(),
                                    1,
                                    deps.signing_key_generator().algorithm(),
                                    key_pair.encrypted_private_key,
                                    key_pair.public_key_pem,
                                    origin.key_id_uri(&key_id),
                                    now,
                                    None,
                                    None,
                                );
                                repository
                                    .create_identity(
                                        executor,
                                        &InstanceIdentity::new(origin.clone(), now),
                                    )
                                    .await?;
                                repository
                                    .create_signing_key(executor, &signing_key)
                                    .await?;
                                Ok(true)
                            }
                        }
                    })
                })
                .await?;
            let unlocked = self.unlock_instance_signing_key().await?;
            Ok(InstanceIdentityReport {
                created,
                key_id_uri: unlocked.key_id_uri().to_string(),
            })
        }
    }
}

impl<T> EnsureInstanceIdentityUseCase for T where
    T: 'static
        + Clone
        + Sync
        + Send
        + DependOnTransactionManager
        + DependOnInstanceIdentityRepository
        + DependOnSigningKeyGenerator
        + DependOnPasswordProvider
        + DependOnPublicBaseUrl
        + UnlockInstanceSigningKeyUseCase
{
}

// ---------------------------------------------------------------------------
// Actor document
// ---------------------------------------------------------------------------

pub trait GetInstanceActorUseCase:
    'static + Sync + Send + DependOnInstanceIdentityRepository + DependOnPublicBaseUrl
{
    /// The public instance actor document: id `{PUBLIC_BASE_URL}/ap/instance`, type `Application`,
    /// and every published (not revoked) public key: the active one and any retired ones. The stored state is validated on every read, so
    /// a damaged or mismatching identity is a 500 rather than a document with wrong URLs.
    fn get_instance_actor(
        &self,
    ) -> impl Future<Output = error_stack::Result<InstanceActor, KernelError>> + Send {
        async move {
            let origin = InstanceOrigin::parse(self.public_base_url().as_str())?;
            let mut executor = self.database_connection().connection().await?;
            let repository = self.instance_identity_repository();
            let identity = repository
                .find_identity(&mut executor)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::Internal)
                        .attach_printable("the instance identity has not been provisioned")
                })?;
            let keys = repository.find_signing_keys(&mut executor).await?;
            drop(executor);
            select_active_key(&origin, &identity, &keys)?;
            Ok(InstanceActor::new(
                origin.actor_id(),
                keys.iter()
                    .filter(|key| key.is_published())
                    .map(|key| (key.key_id_uri.clone(), key.public_key_pem.clone())),
            ))
        }
    }
}

impl<T> GetInstanceActorUseCase for T where
    T: 'static + Sync + Send + DependOnInstanceIdentityRepository + DependOnPublicBaseUrl
{
}

#[cfg(test)]
mod tests;
