//! Emumet's instance identity and instance signing key (P1 post-relay, K1).
//!
//! The instance actor (`{origin}/ap/instance`, an `Application`) is the signer of the requests
//! Emumet sends to a ShuttlePub home. It is not a Profile and its key is never a Profile key: the
//! key lives in its own tables, has its own key id namespace (`#key-{opaque}`, never reused) and
//! is encrypted with the same master key password boundary as the Profile keys.
//!
//! The identity is bound to the **public origin** it was created for. Every URL is derived from
//! that origin (configured `PUBLIC_BASE_URL`), never from an inbound `Host` header. A stored
//! identity whose origin differs from the configured one is an error, not something to repair.
//!
//! Rotation and revocation of the key (K2) are not part of this module: the repository has no
//! operation that revokes a key or adds a second active one.

use crate::crypto::{EncryptedPrivateKey, SigningAlgorithm};
use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::KernelError;
use destructure::Destructure;
use error_stack::Report;
use std::future::Future;
use time::OffsetDateTime;
use vodca::{Newln, References};

/// Path of the instance actor under the public origin.
const INSTANCE_ACTOR_PATH: &str = "/ap/instance";
/// Prefix of the fragment of every instance key id (`{actor}#key-{opaque}`).
const KEY_FRAGMENT_PREFIX: &str = "#key-";

/// A normalized public origin: lowercase `http`/`https` scheme and host, no userinfo, no path,
/// query or fragment, and the default port omitted (`https://emumet.example`).
///
/// `https` is the production form (the shared contract requires it of receivers); `http` is only
/// accepted so that local development with `http://localhost:8080` keeps working.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstanceOrigin(String);

impl InstanceOrigin {
    /// Parses and normalizes a configured public base URL (a trailing `/` is accepted).
    /// Anything that is not a bare origin is a `Validation` error.
    pub fn parse(raw: &str) -> error_stack::Result<Self, KernelError> {
        let reject = |reason: &str| {
            Report::new(KernelError::Validation).attach_printable(format!(
                "PUBLIC_BASE_URL must be a bare http(s) origin ({reason}): {raw}"
            ))
        };
        let url = url::Url::parse(raw.trim()).map_err(|_| reject("not a valid URL"))?;
        if url.scheme() != "https" && url.scheme() != "http" {
            return Err(reject("scheme must be http or https"));
        }
        if url.host_str().is_none() {
            return Err(reject("no host"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(reject("userinfo is not allowed"));
        }
        if url.path() != "/" {
            return Err(reject("path is not allowed"));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(reject("query and fragment are not allowed"));
        }
        // `Origin::ascii_serialization` lowercases scheme and host and omits the default port.
        Ok(Self(url.origin().ascii_serialization()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `{origin}/ap/instance`: the instance actor id and the owner of every instance key.
    pub fn actor_id(&self) -> String {
        format!("{}{INSTANCE_ACTOR_PATH}", self.0)
    }

    /// `{origin}/ap/instance#key-{opaque}`.
    pub fn key_id_uri(&self, key_id: &InstanceSigningKeyId) -> String {
        format!(
            "{}{KEY_FRAGMENT_PREFIX}{}",
            self.actor_id(),
            key_id.as_str()
        )
    }
}

/// The opaque part of an instance key id: 32 lowercase hex characters of random data. It is
/// generated once per key and stored; a key id is never reused (database unique constraint, and
/// rows are never deleted).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstanceSigningKeyId(String);

impl InstanceSigningKeyId {
    const LENGTH: usize = 32;

    pub fn generate() -> Self {
        const HEX: [char; 16] = [
            '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
        ];
        // `nanoid!` needs a single-token size; `LENGTH` is asserted by the unit tests.
        Self(nanoid::nanoid!(32, &HEX))
    }

    /// Accepts only the canonical form (`[0-9a-f]{32}`), e.g. when reading a stored value.
    pub fn parse(raw: &str) -> error_stack::Result<Self, KernelError> {
        if raw.len() == Self::LENGTH && raw.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            Ok(Self(raw.to_string()))
        } else {
            Err(Report::new(KernelError::Internal)
                .attach_printable("Stored instance signing key id is not in canonical form"))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for InstanceSigningKeyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The one instance identity of a deployment (a singleton row, immutable once created).
#[derive(Debug, Clone, References, Newln, Destructure)]
pub struct InstanceIdentity {
    public_origin: InstanceOrigin,
    pub created_at: OffsetDateTime,
}

/// An instance signing key. Signing and publication are separate states:
///
/// * **active** (neither retired nor revoked): the key that signs, and is published. At most one
///   exists (database unique index); K1 creates exactly one and never changes its state;
/// * **retired** (`retired_at` set): no longer signs but is still published, so earlier signatures
///   stay verifiable;
/// * **revoked** (`revoked_at` set): neither signs nor is published.
///
/// Moving a key between these states is K2.
///
/// `revision` is the cache-coherence token of the in-process key cache (cache key = key id +
/// revision): it changes whenever K2 changes the state of a key whose id stays the same. K1
/// never changes it.
#[derive(Debug, Clone, References, Newln, Destructure)]
pub struct InstanceSigningKey {
    id: InstanceSigningKeyId,
    revision: i64,
    algorithm: SigningAlgorithm,
    encrypted_private_key: EncryptedPrivateKey,
    pub public_key_pem: String,
    pub key_id_uri: String,
    pub created_at: OffsetDateTime,
    pub retired_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
}

impl InstanceSigningKey {
    /// The key that signs (not retired, not revoked).
    pub fn is_active(&self) -> bool {
        self.retired_at.is_none() && self.revoked_at.is_none()
    }

    /// Whether the public key is published in the instance actor document (not revoked).
    pub fn is_published(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// Persistence of the instance identity and key.
///
/// Deliberately has no operation that revokes a key, replaces the identity or adds a second
/// active key to an existing identity: those are K2. `create_*` fail with
/// `KernelError::Concurrency` on a unique-constraint conflict instead of replacing anything.
pub trait InstanceIdentityRepository: Sync + Send + 'static {
    type Connection: Connection;

    /// Serializes concurrent provisioning (transaction-scoped advisory lock, released on commit /
    /// rollback). Call first inside the provisioning transaction.
    fn lock_provisioning(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    /// `None` when no identity has been created yet. A stored row that cannot be read back
    /// (corrupt origin) is an `Internal` error, never `None`.
    fn find_identity(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<Option<InstanceIdentity>, KernelError>> + Send;

    /// All keys, retired and revoked ones included, oldest first. A stored row that cannot be read back is an
    /// `Internal` error.
    fn find_signing_keys(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<Vec<InstanceSigningKey>, KernelError>> + Send;

    /// Fails with `Concurrency` when an identity already exists (it is never replaced).
    fn create_identity(
        &self,
        executor: &mut Self::Connection,
        identity: &InstanceIdentity,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    /// Fails with `Concurrency` when an active key already exists or the key id is already
    /// taken, so a second generation never replaces an existing key.
    fn create_signing_key(
        &self,
        executor: &mut Self::Connection,
        signing_key: &InstanceSigningKey,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnInstanceIdentityRepository: Sync + Send + DependOnDatabaseConnection {
    type InstanceIdentityRepository: InstanceIdentityRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn instance_identity_repository(&self) -> &Self::InstanceIdentityRepository;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_is_normalized_and_bare() {
        for (raw, expected) in [
            ("https://emumet.example", "https://emumet.example"),
            ("https://Emumet.Example/", "https://emumet.example"),
            ("https://emumet.example:443", "https://emumet.example"),
            ("http://localhost:8080", "http://localhost:8080"),
            ("http://localhost:80/", "http://localhost"),
            ("https://emumet.example:8443", "https://emumet.example:8443"),
        ] {
            assert_eq!(
                InstanceOrigin::parse(raw).unwrap().as_str(),
                expected,
                "{raw}"
            );
        }
    }

    #[test]
    fn origin_rejects_anything_that_is_not_a_bare_origin() {
        for raw in [
            "",
            "emumet.example",
            "ftp://emumet.example",
            "https://user@emumet.example",
            "https://user:pw@emumet.example",
            "https://emumet.example/emumet",
            "https://emumet.example/?a=b",
            "https://emumet.example/#frag",
            "https://",
        ] {
            let error = InstanceOrigin::parse(raw).unwrap_err();
            assert_eq!(error.current_context(), &KernelError::Validation, "{raw}");
        }
    }

    #[test]
    fn urls_are_derived_from_the_origin_only() {
        let origin = InstanceOrigin::parse("https://emumet.example/").unwrap();
        let opaque = "0123456789abcdef0123456789abcdef";
        let key = InstanceSigningKeyId::parse(opaque).unwrap();
        assert_eq!(origin.actor_id(), "https://emumet.example/ap/instance");
        assert_eq!(
            origin.key_id_uri(&key),
            format!("https://emumet.example/ap/instance#key-{opaque}")
        );
    }

    #[test]
    fn generated_key_ids_are_canonical_and_distinct() {
        let a = InstanceSigningKeyId::generate();
        let b = InstanceSigningKeyId::generate();
        assert_ne!(a, b);
        for id in [&a, &b] {
            assert_eq!(InstanceSigningKeyId::parse(id.as_str()).unwrap(), *id);
        }
    }

    #[test]
    fn non_canonical_key_ids_are_rejected() {
        for raw in [
            "",
            "ABCDEF0123456789abcdef0123456789",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789abcdef0123456789abcdeg",
        ] {
            assert!(InstanceSigningKeyId::parse(raw).is_err(), "{raw}");
        }
    }
}
