//! Profile actor document and WebFinger generation (P1, ADR profile-actor-identity).
//!
//! 1 Profile = 1 actor. The actor id is `{base}/ap/profiles/{profile_nanoid}`, the acct is
//! `username@domain` and the public key is the Profile's own key. These use cases only generate
//! the documents: the routes that serve them are not mounted on the production router before the
//! P4 cutover, and a Profile only becomes an actor once it has both a username and a key.

use crate::dto::activitypub::{GetProfileActorDto, GetProfileWebFingerDto};
use error_stack::Report;
use kernel::activitypub::{Actor, ActorImages, ActorUrlBuilder, WebFingerLink, WebFingerResponse};
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::read_model::{DependOnProfileQuery, ProfileProjection, ProfileQuery};
use kernel::interfaces::repository::{
    DependOnImageRepository, DependOnProfileSigningKeyRepository, ImageRepository,
};
use kernel::prelude::entity::{
    ImageId, Nanoid, Profile, ProfileSigningKey, ProfileSigningKeyRepository, ProfileUsername,
};
use kernel::KernelError;
use std::future::Future;

fn not_an_actor(profile: &ProfileProjection, reason: &str) -> Report<KernelError> {
    Report::new(KernelError::NotFound).attach_printable(format!(
        "Profile {} is not an actor yet: {reason}",
        profile.nanoid().as_ref()
    ))
}

async fn active_key<T>(
    deps: &T,
    executor: &mut <<T as kernel::interfaces::database::DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    profile: &ProfileProjection,
) -> error_stack::Result<ProfileSigningKey, KernelError>
where
    T: ?Sized + DependOnProfileSigningKeyRepository,
{
    deps.profile_signing_key_repository()
        .find_active_by_profile_id(executor, profile.id())
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| not_an_actor(profile, "no active signing key"))
}

pub trait GetProfileActorUseCase:
    'static
    + Sync
    + Send
    + DependOnProfileQuery
    + DependOnProfileSigningKeyRepository
    + DependOnImageRepository
    + DependOnPublicBaseUrl
{
    fn get_profile_actor(
        &self,
        dto: GetProfileActorDto,
    ) -> impl Future<Output = error_stack::Result<Actor, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().connection().await?;
            let nanoid = Nanoid::<Profile>::new(dto.profile_nanoid);
            let profile = self
                .profile_query()
                .find_by_nanoid(&mut executor, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable(format!("Profile not found: {}", nanoid.as_ref()))
                })?;
            let username = profile
                .username()
                .as_ref()
                .ok_or_else(|| not_an_actor(&profile, "no username"))?;
            let signing_key = active_key(self, &mut executor, &profile).await?;

            let image_ids: Vec<ImageId> = profile
                .icon()
                .iter()
                .chain(profile.banner().iter())
                .cloned()
                .collect();
            let images = self
                .image_repository()
                .find_by_ids(&mut executor, &image_ids)
                .await?;
            let image_url = |id: &Option<ImageId>| {
                id.as_ref().and_then(|id| {
                    images
                        .iter()
                        .find(|image| image.id() == id)
                        .map(|image| image.url().as_ref().to_string())
                })
            };
            let icon_url = image_url(profile.icon());
            let banner_url = image_url(profile.banner());

            Ok(Actor::new(
                &ActorUrlBuilder::for_profile(
                    self.public_base_url().as_str(),
                    profile.nanoid().as_ref(),
                ),
                username.as_ref(),
                profile.display_name().as_ref().map(|v| v.as_ref().as_str()),
                profile.summary().as_ref().map(|v| v.as_ref().as_str()),
                &ActorImages {
                    icon_url: icon_url.as_deref(),
                    banner_url: banner_url.as_deref(),
                },
                &signing_key.public_key_pem,
                &signing_key.key_id_uri,
            ))
        }
    }
}

impl<T> GetProfileActorUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnProfileQuery
        + DependOnProfileSigningKeyRepository
        + DependOnImageRepository
        + DependOnPublicBaseUrl
{
}

pub trait GetProfileWebFingerUseCase:
    'static
    + Sync
    + Send
    + DependOnProfileQuery
    + DependOnProfileSigningKeyRepository
    + DependOnPublicBaseUrl
{
    /// Resolves `acct:username@domain` to the Profile actor. The caller is responsible for
    /// checking that `domain` is this instance's domain. Unknown or malformed usernames, Profiles
    /// without a key, and deleted Profiles are all `NotFound`.
    fn get_profile_webfinger(
        &self,
        dto: GetProfileWebFingerDto,
    ) -> impl Future<Output = error_stack::Result<WebFingerResponse, KernelError>> + Send {
        async move {
            ProfileUsername::check(&dto.username).map_err(|e| {
                Report::new(KernelError::NotFound).attach_printable(e.message().to_string())
            })?;
            let username = ProfileUsername::new(dto.username);
            let mut executor = self.database_connection().connection().await?;
            let profile = self
                .profile_query()
                .find_by_username(&mut executor, &username)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Profile not found with username: {}",
                        username.as_ref()
                    ))
                })?;
            active_key(self, &mut executor, &profile).await?;
            let actor_url = ActorUrlBuilder::for_profile(
                self.public_base_url().as_str(),
                profile.nanoid().as_ref(),
            )
            .actor_id();

            Ok(WebFingerResponse {
                subject: format!(
                    "acct:{}@{}",
                    username.as_ref(),
                    dto.domain.to_ascii_lowercase()
                ),
                links: Some(vec![WebFingerLink {
                    rel: "self".to_string(),
                    type_: "application/activity+json".to_string(),
                    href: actor_url.clone(),
                }]),
                aliases: Some(vec![actor_url]),
            })
        }
    }
}

impl<T> GetProfileWebFingerUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnProfileQuery
        + DependOnProfileSigningKeyRepository
        + DependOnPublicBaseUrl
{
}
