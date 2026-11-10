//! Profile-owned actor signing keys (P1, ADR profile-actor-identity).
//!
//! These use cases are building blocks for the Profile federation stack (P3o/P3i/P4). Nothing in
//! the production server calls [`CreateProfileSigningKeyUseCase`]: the one-time generation of
//! Profile keys belongs to the P4 cutover. They are intentionally not exposed through any HTTP
//! route and no generic "sign anything" API is built on top of them.

use error_stack::Report;
use kernel::activitypub::ActorUrlBuilder;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::crypto::{
    DependOnKeyEncryptor, DependOnPasswordProvider, DependOnSigningKeyGenerator, KeyEncryptor,
    PasswordProvider, SigningAlgorithm, SigningKeyGenerator,
};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::http_signing::{
    DependOnHttpSigner, HttpSigner, HttpSigningRequest, HttpSigningResponse,
};
use kernel::interfaces::repository::DependOnProfileSigningKeyRepository;
use kernel::prelude::entity::{
    Nanoid, Profile, ProfileId, ProfileSigningKey, ProfileSigningKeyRepository, SigningKeyId,
};
use kernel::KernelError;
use std::future::Future;

use crate::signing_key::PublicKeyInfo;

pub trait CreateProfileSigningKeyUseCase:
    'static
    + Sync
    + Send
    + DependOnProfileSigningKeyRepository
    + DependOnSigningKeyGenerator
    + DependOnPublicBaseUrl
    + DependOnPasswordProvider
{
    /// Generate and store the Profile's key. Fails with `Concurrency` when the Profile already
    /// has an active key (database unique index), so a second generation never silently
    /// replaces an existing identity key.
    fn create_profile_signing_key(
        &self,
        executor: &mut <<Self as kernel::interfaces::database::DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
        profile_id: ProfileId,
        profile_nanoid: &Nanoid<Profile>,
        algorithm: SigningAlgorithm,
    ) -> impl Future<Output = error_stack::Result<ProfileSigningKey, KernelError>> + Send {
        async move {
            let password = self.password_provider().get_password()?;
            let key_pair = self.signing_key_generator().generate(&password)?;
            let key_id_uri = ActorUrlBuilder::for_profile(
                self.public_base_url().as_str(),
                profile_nanoid.as_ref(),
            )
            .key_id();
            let signing_key = ProfileSigningKey::new(
                SigningKeyId::default(),
                profile_id,
                algorithm,
                key_pair.encrypted_private_key,
                key_pair.public_key_pem,
                key_id_uri,
                time::OffsetDateTime::now_utc(),
                None,
            );
            self.profile_signing_key_repository()
                .create(executor, &signing_key)
                .await?;
            Ok(signing_key)
        }
    }
}

impl<T: ?Sized> CreateProfileSigningKeyUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnProfileSigningKeyRepository
        + DependOnSigningKeyGenerator
        + DependOnPublicBaseUrl
        + DependOnPasswordProvider
{
}

pub trait GetProfilePublicKeyUseCase:
    'static + Sync + Send + DependOnProfileSigningKeyRepository + DependOnPublicBaseUrl
{
    fn get_profile_public_key_info(
        &self,
        profile_id: &ProfileId,
        profile_nanoid: &Nanoid<Profile>,
    ) -> impl Future<Output = error_stack::Result<PublicKeyInfo, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().connection().await?;
            let key = self
                .profile_signing_key_repository()
                .find_active_by_profile_id(&mut executor, profile_id)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable("No active signing key found for profile")
                })?;
            let owner = ActorUrlBuilder::for_profile(
                self.public_base_url().as_str(),
                profile_nanoid.as_ref(),
            )
            .actor_id();
            Ok(PublicKeyInfo {
                id: key.key_id_uri,
                owner,
                public_key_pem: key.public_key_pem,
            })
        }
    }
}

impl<T> GetProfilePublicKeyUseCase for T where
    T: 'static + Sync + Send + DependOnProfileSigningKeyRepository + DependOnPublicBaseUrl
{
}

/// Signs an outgoing HTTP request with the Profile's active key. Internal building block for the
/// Profile delivery worker (P3o); not reachable from any route.
pub trait SignProfileRequestUseCase:
    'static
    + Sync
    + Send
    + DependOnProfileSigningKeyRepository
    + DependOnHttpSigner
    + DependOnPasswordProvider
    + DependOnKeyEncryptor
{
    fn sign_as_profile(
        &self,
        profile_id: &ProfileId,
        request: HttpSigningRequest,
    ) -> impl Future<Output = error_stack::Result<HttpSigningResponse, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().connection().await?;
            let key = self
                .profile_signing_key_repository()
                .find_active_by_profile_id(&mut executor, profile_id)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable("No active signing key found for profile")
                })?;
            let password = self.password_provider().get_password()?;
            let private_key_pem = self
                .key_encryptor()
                .decrypt(key.encrypted_private_key(), &password)?;
            self.http_signer()
                .sign(&request, &private_key_pem, &key.key_id_uri, key.algorithm())
                .await
        }
    }
}

impl<T> SignProfileRequestUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnProfileSigningKeyRepository
        + DependOnHttpSigner
        + DependOnPasswordProvider
        + DependOnKeyEncryptor
{
}
