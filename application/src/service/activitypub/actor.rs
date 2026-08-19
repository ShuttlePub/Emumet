use crate::dto::activitypub::{GetActorDto, GetWebFingerDto};
use error_stack::Report;
use kernel::activitypub::{Actor, ActorImages, ActorUrlBuilder, WebFingerLink, WebFingerResponse};
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::read_model::{AccountQuery, DependOnAccountQuery};
use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
use kernel::interfaces::repository::{
    DependOnImageRepository, DependOnSigningKeyRepository, ImageRepository, SigningKeyRepository,
};
use kernel::prelude::entity::{Account, AccountName, ImageId, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait GetActorUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQuery
    + DependOnProfileReadModel
    + DependOnSigningKeyRepository
    + DependOnImageRepository
    + DependOnPublicBaseUrl
{
    fn get_actor(
        &self,
        dto: GetActorDto,
    ) -> impl Future<Output = error_stack::Result<Actor, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().connection().await?;
            let account_nanoid = Nanoid::<Account>::new(dto.account_nanoid);
            let account = self
                .account_query()
                .find_by_nanoid(&mut executor, &account_nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        account_nanoid.as_ref()
                    ))
                })?;
            let profile = self
                .profile_read_model()
                .find_by_account_id(&mut executor, account.id())
                .await?;
            let signing_key = self
                .signing_key_repository()
                .find_active_by_account_id(&mut executor, account.id())
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable("No active signing key found for account")
                })?;
            let display_name = profile
                .as_ref()
                .and_then(|profile| profile.display_name().as_ref())
                .map(|display_name| display_name.as_ref().to_string());
            let summary = profile
                .as_ref()
                .and_then(|profile| profile.summary().as_ref())
                .map(|summary| summary.as_ref().to_string());
            let image_ids: Vec<ImageId> = profile
                .as_ref()
                .into_iter()
                .flat_map(|profile| {
                    profile
                        .icon()
                        .iter()
                        .chain(profile.banner().iter())
                        .cloned()
                })
                .collect();
            let images = self
                .image_repository()
                .find_by_ids(&mut executor, &image_ids)
                .await?;
            let image_url = |id: Option<&ImageId>| {
                id.and_then(|id| {
                    images
                        .iter()
                        .find(|image| image.id() == id)
                        .map(|image| image.url().as_ref())
                })
            };
            let icon_url = profile
                .as_ref()
                .and_then(|profile| image_url(profile.icon().as_ref()));
            let banner_url = profile
                .as_ref()
                .and_then(|profile| image_url(profile.banner().as_ref()));

            Ok(Actor::new(
                &ActorUrlBuilder::new(self.public_base_url().as_str(), account.nanoid().as_ref()),
                account.name().as_ref(),
                display_name.as_deref(),
                summary.as_deref(),
                &ActorImages {
                    icon_url: icon_url.map(String::as_str),
                    banner_url: banner_url.map(String::as_str),
                },
                &signing_key.public_key_pem,
                &signing_key.key_id_uri,
            ))
        }
    }
}

impl<T> GetActorUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQuery
        + DependOnProfileReadModel
        + DependOnSigningKeyRepository
        + DependOnImageRepository
        + DependOnPublicBaseUrl
{
}

pub trait GetWebFingerUseCase:
    'static + Sync + Send + DependOnAccountQuery + DependOnPublicBaseUrl
{
    fn get_webfinger(
        &self,
        dto: GetWebFingerDto,
    ) -> impl Future<Output = error_stack::Result<WebFingerResponse, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().connection().await?;
            let account_name = AccountName::new(dto.account_name);
            account_name.validate()?;
            let account = self
                .account_query()
                .find_by_name(&mut executor, &account_name)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with name: {}",
                        account_name.as_ref()
                    ))
                })?;
            let actor_url =
                ActorUrlBuilder::new(self.public_base_url().as_str(), account.nanoid().as_ref())
                    .actor_id();

            Ok(WebFingerResponse {
                subject: format!(
                    "acct:{}@{}",
                    account.name().as_ref(),
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

impl<T> GetWebFingerUseCase for T where
    T: 'static + Sync + Send + DependOnAccountQuery + DependOnPublicBaseUrl
{
}
