use crate::transfer::activitypub::{
    GetActorDto, GetWebFingerDto, InboxActivityDto, SendFollowDto, SendFollowResultDto,
};
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use base64::{engine::general_purpose, Engine as _};
use error_stack::{Report, ResultExt};
use kernel::activitypub::{Activity, Actor, OrderedCollection, WebFingerLink, WebFingerResponse};
use kernel::interfaces::config::{DependOnPublicBaseUrl, PublicBaseUrl};
use kernel::interfaces::crypto::{
    DependOnKeyEncryptor, DependOnPasswordProvider, KeyEncryptor, PasswordProvider,
};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::http_signing::{DependOnHttpSigner, HttpSigner, HttpSigningRequest};
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
use kernel::interfaces::repository::{
    DependOnFollowRepository, DependOnOutboxActivityRepository, DependOnRemoteAccountRepository,
    DependOnSigningKeyRepository, FollowRepository, OutboxActivityRepository,
    RemoteAccountRepository, SigningKeyRepository,
};
use kernel::prelude::entity::{
    Account, AccountId, AccountName, Follow, FollowApprovedAt, FollowId, FollowTargetId, Nanoid,
    OutboxActivity, OutboxActivityId, RemoteAccount, RemoteAccountAcct, RemoteAccountId,
    RemoteAccountUrl,
};
use kernel::KernelError;
use reqwest::header::{ACCEPT, CONTENT_TYPE, DATE, HOST, USER_AGENT};
use serde_json::Value;
use sha2::Digest;
use std::future::Future;
#[cfg(not(test))]
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
#[cfg(test)]
use std::net::{IpAddr, SocketAddr};

const ACTIVITY_JSON: &str = "application/activity+json";
const ACTIVITYSTREAMS_CONTEXT: &str = "https://www.w3.org/ns/activitystreams";

pub trait GetActorUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnProfileReadModel
    + DependOnSigningKeyRepository
    + DependOnPublicBaseUrl
{
    fn get_actor(
        &self,
        dto: GetActorDto,
    ) -> impl Future<Output = error_stack::Result<Actor, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().get_executor().await?;
            let account_nanoid = Nanoid::<Account>::new(dto.account_nanoid);
            let account = self
                .account_query_processor()
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

            Ok(Actor::new(
                self.public_base_url().as_str(),
                account.nanoid().as_ref(),
                account.name().as_ref(),
                display_name.as_deref(),
                summary.as_deref(),
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
        + DependOnAccountQueryProcessor
        + DependOnProfileReadModel
        + DependOnSigningKeyRepository
        + DependOnPublicBaseUrl
{
}

pub trait GetWebFingerUseCase:
    'static + Sync + Send + DependOnAccountQueryProcessor + DependOnPublicBaseUrl
{
    fn get_webfinger(
        &self,
        dto: GetWebFingerDto,
    ) -> impl Future<Output = error_stack::Result<WebFingerResponse, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().get_executor().await?;
            let account_name = AccountName::new(dto.account_name);
            account_name.validate()?;
            let account = self
                .account_query_processor()
                .find_by_name(&mut executor, &account_name)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with name: {}",
                        account_name.as_ref()
                    ))
                })?;
            let actor_url = format!(
                "{}/accounts/{}",
                self.public_base_url().as_str(),
                account.nanoid().as_ref()
            );

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
    T: 'static + Sync + Send + DependOnAccountQueryProcessor + DependOnPublicBaseUrl
{
}

pub trait GetFollowersCollectionUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnFollowRepository
    + DependOnPublicBaseUrl
{
    fn get_followers_collection(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<OrderedCollection, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().get_executor().await?;
            let account = self
                .account_query_processor()
                .find_by_id(&mut executor, account_id)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable(format!("Account not found: {account_id:?}"))
                })?;
            let follows = self
                .follow_repository()
                .find_followers(&mut executor, &FollowTargetId::from(account_id.clone()))
                .await?;
            let total_items = follows
                .iter()
                .filter(|follow| follow.approved_at().is_some())
                .count() as u64;

            Ok(OrderedCollection::new(
                format!(
                    "{}/accounts/{}/followers",
                    self.public_base_url().as_str(),
                    account.nanoid().as_ref()
                ),
                total_items,
                None,
                None,
            ))
        }
    }

    fn get_following_collection(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<OrderedCollection, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().get_executor().await?;
            let account = self
                .account_query_processor()
                .find_by_id(&mut executor, account_id)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable(format!("Account not found: {account_id:?}"))
                })?;
            let follows = self
                .follow_repository()
                .find_followings(&mut executor, &FollowTargetId::from(account_id.clone()))
                .await?;
            let total_items = follows
                .iter()
                .filter(|follow| follow.approved_at().is_some())
                .count() as u64;

            Ok(OrderedCollection::new(
                format!(
                    "{}/accounts/{}/following",
                    self.public_base_url().as_str(),
                    account.nanoid().as_ref()
                ),
                total_items,
                None,
                None,
            ))
        }
    }
}

impl<T> GetFollowersCollectionUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnFollowRepository
        + DependOnPublicBaseUrl
{
}

pub trait StoreOutboxActivityUseCase:
    'static + Sync + Send + DependOnOutboxActivityRepository
{
    fn store_outbox_activity(
        &self,
        activity: &OutboxActivity,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().get_executor().await?;
            self.outbox_activity_repository()
                .create(&mut executor, activity)
                .await
        }
    }
}

impl<T> StoreOutboxActivityUseCase for T where
    T: 'static + Sync + Send + DependOnOutboxActivityRepository
{
}

pub trait GetOutboxUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnOutboxActivityRepository
    + DependOnPublicBaseUrl
{
    fn get_outbox_collection(
        &self,
        account_id: &AccountId,
        limit: usize,
        cursor: Option<i64>,
    ) -> impl Future<Output = error_stack::Result<OrderedCollection, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().get_executor().await?;
            let account = self
                .account_query_processor()
                .find_by_id(&mut executor, account_id)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable(format!("Account not found: {account_id:?}"))
                })?;
            let activities = self
                .outbox_activity_repository()
                .find_by_account_id(&mut executor, account_id, limit, cursor)
                .await?;
            let total_items = self
                .outbox_activity_repository()
                .count_by_account_id(&mut executor, account_id)
                .await?;
            let ordered_items = activities
                .into_iter()
                .map(|activity| {
                    serde_json::from_str::<serde_json::Value>(&activity.object_json).map_err(|e| {
                        Report::new(KernelError::Internal).attach_printable(format!(
                            "Failed to deserialize outbox activity JSON: {e}"
                        ))
                    })
                })
                .collect::<error_stack::Result<Vec<_>, KernelError>>()?;

            Ok(OrderedCollection::with_ordered_items(
                format!(
                    "{}/accounts/{}/outbox",
                    self.public_base_url().as_str(),
                    account.nanoid().as_ref()
                ),
                total_items,
                ordered_items,
            ))
        }
    }
}

impl<T> GetOutboxUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnOutboxActivityRepository
        + DependOnPublicBaseUrl
{
}

pub trait SendFollowUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnFollowRepository
    + DependOnRemoteAccountRepository
    + DependOnSigningKeyRepository
    + DependOnHttpSigner
    + DependOnPasswordProvider
    + DependOnKeyEncryptor
    + DependOnPublicBaseUrl
    + DependOnOutboxActivityRepository
    + DependOnPermissionChecker
    + StoreOutboxActivityUseCase
{
    fn send_follow(
        &self,
        auth_account_id: kernel::prelude::entity::AuthAccountId,
        dto: SendFollowDto,
    ) -> impl Future<Output = error_stack::Result<SendFollowResultDto, KernelError>> + Send
    where
        Self: Sized,
    {
        async move {
            let account_nanoid = Nanoid::<Account>::new(dto.account_nanoid.clone());
            let mut executor = self.database_connection().get_executor().await?;
            let account = self
                .account_query_processor()
                .find_by_nanoid(&mut executor, &account_nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        account_nanoid.as_ref()
                    ))
                })?;

            crate::permission::check_permission(
                self,
                &auth_account_id,
                &crate::permission::account_sign(account.id()),
            )
            .await?;

            let remote_actor = resolve_remote_actor_identifier(&dto.target).await?;
            let remote_account = upsert_remote_account(
                self.remote_account_repository(),
                &mut executor,
                remote_actor,
            )
            .await?;

            let source = FollowTargetId::from(account.id().clone());
            let destination = FollowTargetId::from(remote_account.id().clone());

            if let Some(_existing) = find_existing_following(
                self.follow_repository(),
                &mut executor,
                &source,
                &destination,
            )
            .await?
            {
                return Err(Report::new(KernelError::Rejected).attach_printable(format!(
                    "Already following {}",
                    remote_account.url().as_ref()
                )));
            }

            let follow = Follow::new(
                FollowId::new(kernel::generate_id()),
                source,
                destination,
                None,
            )?;
            self.follow_repository()
                .create(&mut executor, &follow)
                .await?;

            let local_actor_url =
                local_actor_url(self.public_base_url(), account.nanoid().as_ref());
            let follow_activity = follow_activity(
                self.public_base_url(),
                &follow,
                &local_actor_url,
                remote_account.url().as_ref(),
            )?;

            if let Err(error) = self
                .deliver_follow(account.id(), remote_account.inbox_url(), &follow_activity)
                .await
            {
                self.follow_repository()
                    .delete(&mut executor, follow.id())
                    .await?;
                return Err(error
                    .change_context(KernelError::Rejected)
                    .attach_printable("Follow delivery failed, rolled back follow record"));
            }

            let outbox_entry = OutboxActivity {
                id: OutboxActivityId::default(),
                account_id: account.id().clone(),
                activity_id: follow_activity.id.clone(),
                activity_type: "Follow".to_string(),
                object_json: serde_json::to_string(&follow_activity).map_err(|e| {
                    Report::new(KernelError::Internal).attach_printable(format!(
                        "Failed to serialize Follow activity to JSON: {e}"
                    ))
                })?,
                created_at: time::OffsetDateTime::now_utc(),
            };
            self.store_outbox_activity(&outbox_entry)
                .await
                .change_context_lazy(|| KernelError::Internal)
                .attach_printable("Failed to store outbox activity")?;

            Ok(SendFollowResultDto {
                follow_id: follow.id().as_ref().to_string(),
                remote_actor_url: remote_account.url().as_ref().to_string(),
                activity_id: follow_activity.id.clone(),
                approved: false,
            })
        }
    }

    fn deliver_follow(
        &self,
        account_id: &AccountId,
        inbox_url: &Option<String>,
        activity: &Activity,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send
    where
        Self: Sized,
    {
        async move {
            let inbox_url = inbox_url.as_deref().ok_or_else(|| {
                Report::new(KernelError::Rejected)
                    .attach_printable("Remote actor does not expose an inbox URL")
            })?;
            deliver_activity_to_inbox(
                self.database_connection(),
                self.signing_key_repository(),
                self.password_provider(),
                self.key_encryptor(),
                self.http_signer(),
                account_id,
                inbox_url,
                activity,
                "Follow",
            )
            .await
        }
    }
}

impl<T> SendFollowUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnFollowRepository
        + DependOnRemoteAccountRepository
        + DependOnSigningKeyRepository
        + DependOnHttpSigner
        + DependOnPasswordProvider
        + DependOnKeyEncryptor
        + DependOnPublicBaseUrl
        + DependOnOutboxActivityRepository
        + DependOnPermissionChecker
        + StoreOutboxActivityUseCase
{
}

pub trait InboxUseCase:
    'static
    + Sync
    + Send
    + DependOnFollowRepository
    + DependOnRemoteAccountRepository
    + DependOnSigningKeyRepository
    + DependOnHttpSigner
    + DependOnPasswordProvider
    + DependOnKeyEncryptor
    + DependOnPublicBaseUrl
    + DependOnOutboxActivityRepository
    + StoreOutboxActivityUseCase
{
    fn handle_inbox_activity(
        &self,
        dto: InboxActivityDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            match dto.activity.type_.as_str() {
                "Follow" => self.handle_follow_activity(dto).await,
                "Accept" => self.handle_accept_activity(dto).await,
                "Undo" if undo_object_is_follow(&dto.activity) => {
                    self.handle_undo_follow(dto).await
                }
                activity_type => {
                    tracing::info!(
                        activity_type,
                        "Ignoring unsupported ActivityPub inbox activity"
                    );
                    Ok(())
                }
            }
        }
    }

    fn handle_follow_activity(
        &self,
        dto: InboxActivityDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let followed_actor_url = activity_object_id(&dto.activity).ok_or_else(|| {
                Report::new(KernelError::Rejected)
                    .attach_printable("Follow activity object must be an actor id")
            })?;
            ensure_local_actor_matches(
                self.public_base_url(),
                &dto.account_nanoid,
                &followed_actor_url,
            )?;

            let remote_actor = resolve_remote_actor(&dto.activity.actor).await?;
            let mut executor = self.database_connection().get_executor().await?;
            let remote_account = upsert_remote_account(
                self.remote_account_repository(),
                &mut executor,
                remote_actor,
            )
            .await?;

            let source = FollowTargetId::from(remote_account.id().clone());
            let destination = FollowTargetId::from(dto.account_id.clone());
            let follow = match find_existing_follow(
                self.follow_repository(),
                &mut executor,
                &source,
                &destination,
            )
            .await?
            {
                Some(_existing) => {
                    tracing::debug!("Follow already exists, skipping Accept creation");
                    return Ok(());
                }
                None => {
                    let follow = Follow::new(
                        FollowId::new(kernel::generate_id()),
                        source,
                        destination,
                        Some(FollowApprovedAt::default()),
                    )?;
                    self.follow_repository()
                        .create(&mut executor, &follow)
                        .await?;
                    follow
                }
            };

            let local_actor_url = local_actor_url(self.public_base_url(), &dto.account_nanoid);
            let accept = accept_activity(
                self.public_base_url(),
                &follow,
                &local_actor_url,
                dto.activity.clone(),
            )?;
            if let Err(error) = self
                .deliver_accept(&dto.account_id, remote_account.inbox_url(), &accept)
                .await
            {
                tracing::warn!(?error, inbox_url = ?remote_account.inbox_url(), "Failed to deliver ActivityPub Accept");
            }

            let outbox_entry = OutboxActivity {
                id: OutboxActivityId::default(),
                account_id: dto.account_id.clone(),
                activity_id: accept.id.clone(),
                activity_type: "Accept".to_string(),
                object_json: serde_json::to_string(&accept).map_err(|e| {
                    Report::new(KernelError::Internal).attach_printable(format!(
                        "Failed to serialize Accept activity to JSON: {e}"
                    ))
                })?,
                created_at: time::OffsetDateTime::now_utc(),
            };
            self.store_outbox_activity(&outbox_entry)
                .await
                .change_context_lazy(|| KernelError::Internal)
                .attach_printable("Failed to store outbox activity")?;

            Ok(())
        }
    }

    fn handle_undo_follow(
        &self,
        dto: InboxActivityDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let follow_activity = undo_follow_object(&dto.activity).ok_or_else(|| {
                Report::new(KernelError::Rejected)
                    .attach_printable("Undo activity object must be a Follow activity")
            })?;
            let followed_actor_url = activity_object_id(&follow_activity).ok_or_else(|| {
                Report::new(KernelError::Rejected)
                    .attach_printable("Undo Follow object must target an actor id")
            })?;
            ensure_local_actor_matches(
                self.public_base_url(),
                &dto.account_nanoid,
                &followed_actor_url,
            )?;

            let mut executor = self.database_connection().get_executor().await?;
            let remote_url = RemoteAccountUrl::new(dto.activity.actor.clone());
            let Some(remote_account) = self
                .remote_account_repository()
                .find_by_url(&mut executor, &remote_url)
                .await?
            else {
                return Ok(());
            };

            let source = FollowTargetId::from(remote_account.id().clone());
            let destination = FollowTargetId::from(dto.account_id);
            if let Some(follow) = find_existing_follow(
                self.follow_repository(),
                &mut executor,
                &source,
                &destination,
            )
            .await?
            {
                self.follow_repository()
                    .delete(&mut executor, follow.id())
                    .await?;
            }
            Ok(())
        }
    }

    fn handle_accept_activity(
        &self,
        dto: InboxActivityDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let accept = &dto.activity;
            let nested_follow = accept
                .object
                .as_ref()
                .and_then(|obj| serde_json::from_value::<Activity>(obj.clone()).ok())
                .filter(|a| a.type_ == "Follow")
                .ok_or_else(|| {
                    Report::new(KernelError::Rejected)
                        .attach_printable("Accept object must be a Follow activity")
                })?;

            let follow_actor_url = nested_follow.actor.trim_end_matches('/').to_string();
            let expected_local = local_actor_url(self.public_base_url(), &dto.account_nanoid);
            if follow_actor_url != expected_local.trim_end_matches('/') {
                tracing::debug!(
                    follow_actor = %nested_follow.actor,
                    expected = %expected_local,
                    "Accept Follow actor does not match local actor"
                );
                return Ok(());
            }

            let remote_actor_url = activity_object_id(&nested_follow).ok_or_else(|| {
                Report::new(KernelError::Rejected)
                    .attach_printable("Accept Follow object must have an actor id")
            })?;

            let accept_actor = accept.actor.trim_end_matches('/').to_string();
            if accept_actor != remote_actor_url.trim_end_matches('/') {
                tracing::debug!(
                    accept_actor = %accept.actor,
                    remote_actor = %remote_actor_url,
                    "Accept actor does not match Follow object"
                );
                return Ok(());
            }

            let mut executor = self.database_connection().get_executor().await?;
            let remote_url = RemoteAccountUrl::new(remote_actor_url.clone());
            let remote_account = self
                .remote_account_repository()
                .find_by_url(&mut executor, &remote_url)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Remote account not found for {remote_actor_url}"
                    ))
                })?;

            let source = FollowTargetId::from(dto.account_id.clone());
            let destination = FollowTargetId::from(remote_account.id().clone());
            if let Some(existing) = find_existing_following(
                self.follow_repository(),
                &mut executor,
                &source,
                &destination,
            )
            .await?
            {
                if existing.approved_at().is_none() {
                    let approved = Follow::new(
                        existing.id().clone(),
                        existing.source().clone(),
                        existing.destination().clone(),
                        Some(FollowApprovedAt::default()),
                    )?;
                    self.follow_repository()
                        .update(&mut executor, &approved)
                        .await?;
                    tracing::info!(
                        remote_actor = %remote_actor_url,
                        "Follow approved via Accept activity"
                    );
                }
            } else {
                tracing::debug!(
                    remote_actor = %remote_actor_url,
                    "No pending follow found for Accept activity"
                );
            }
            Ok(())
        }
    }

    fn deliver_accept(
        &self,
        account_id: &AccountId,
        inbox_url: &Option<String>,
        accept: &Activity,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let inbox_url = inbox_url.as_deref().ok_or_else(|| {
                Report::new(KernelError::Rejected)
                    .attach_printable("Remote actor does not expose an inbox URL")
            })?;
            deliver_activity_to_inbox(
                self.database_connection(),
                self.signing_key_repository(),
                self.password_provider(),
                self.key_encryptor(),
                self.http_signer(),
                account_id,
                inbox_url,
                accept,
                "Accept",
            )
            .await
        }
    }
}

impl<T> InboxUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnFollowRepository
        + DependOnRemoteAccountRepository
        + DependOnSigningKeyRepository
        + DependOnHttpSigner
        + DependOnPasswordProvider
        + DependOnKeyEncryptor
        + DependOnPublicBaseUrl
        + DependOnOutboxActivityRepository
        + StoreOutboxActivityUseCase
{
}

#[derive(Debug)]
struct ResolvedRemoteActor {
    acct: RemoteAccountAcct,
    url: RemoteAccountUrl,
    inbox_url: Option<String>,
    public_key_pem: Option<String>,
}

async fn resolve_remote_actor(
    actor_url: &str,
) -> error_stack::Result<ResolvedRemoteActor, KernelError> {
    let mut url = reqwest::Url::parse(actor_url).map_err(|e| {
        Report::new(KernelError::Rejected).attach_printable(format!("Invalid actor URL: {e}"))
    })?;
    let mut redirects = 0;
    let actor = loop {
        if redirects > 5 {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("Too many redirects while resolving remote actor"));
        }
        let resolved_addresses = validate_fetch_url(&url).await?;
        let response = client_for_url(&url, &resolved_addresses)?
            .get(url.clone())
            .header(
                ACCEPT,
                "application/activity+json, application/ld+json, application/json;q=0.9",
            )
            .header(USER_AGENT, "Emumet/0.1 ActivityPub actor resolver")
            .send()
            .await
            .map_err(|e| {
                Report::new(KernelError::Rejected)
                    .attach_printable(format!("Remote actor fetch failed: {e}"))
            })?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    Report::new(KernelError::Rejected)
                        .attach_printable("Remote actor redirect without Location header")
                })?;
            url = url.join(location).map_err(|e| {
                Report::new(KernelError::Rejected)
                    .attach_printable(format!("Remote actor redirect URL is invalid: {e}"))
            })?;
            redirects += 1;
            continue;
        }
        if !response.status().is_success() {
            return Err(Report::new(KernelError::Rejected).attach_printable(format!(
                "Remote actor endpoint returned {}",
                response.status()
            )));
        }
        break response.json::<Actor>().await.map_err(|e| {
            Report::new(KernelError::Rejected)
                .attach_printable(format!("Remote actor JSON is invalid: {e}"))
        })?;
    };

    let actor_id = reqwest::Url::parse(&actor.id).map_err(|e| {
        Report::new(KernelError::Rejected)
            .attach_printable(format!("Remote actor id is invalid: {e}"))
    })?;
    let host = actor_id.host_str().ok_or_else(|| {
        Report::new(KernelError::Rejected).attach_printable("Remote actor id has no host")
    })?;
    if actor.preferred_username.trim().is_empty() {
        return Err(Report::new(KernelError::Rejected)
            .attach_printable("Remote actor preferredUsername is empty"));
    }
    if !same_activitypub_id(&actor.id, actor_url) {
        return Err(Report::new(KernelError::Rejected).attach_printable(format!(
            "Remote actor id does not match requested actor URL: expected {actor_url}, got {}",
            actor.id
        )));
    }

    Ok(ResolvedRemoteActor {
        acct: RemoteAccountAcct::new(format!("{}@{}", actor.preferred_username, host)),
        url: RemoteAccountUrl::new(actor.id),
        inbox_url: Some(actor.inbox),
        public_key_pem: Some(actor.public_key.public_key_pem),
    })
}

async fn resolve_remote_actor_identifier(
    identifier: &str,
) -> error_stack::Result<ResolvedRemoteActor, KernelError> {
    if let Some((user, domain)) = identifier
        .strip_prefix("acct:")
        .and_then(|s| s.split_once('@'))
    {
        if user.is_empty() || domain.is_empty() {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("Invalid acct: format, expected acct:user@domain"));
        }
        return resolve_remote_webfinger(user, domain).await;
    }
    resolve_remote_actor(identifier).await
}

async fn resolve_remote_webfinger(
    user: &str,
    domain: &str,
) -> error_stack::Result<ResolvedRemoteActor, KernelError> {
    if !user
        .chars()
        .all(|c| c.is_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(Report::new(KernelError::Rejected)
            .attach_printable("Invalid characters in WebFinger user"));
    }
    if !domain
        .chars()
        .all(|c| c.is_alphanumeric() || c == '.' || c == '-')
        || domain.starts_with('-')
        || domain.ends_with('-')
    {
        return Err(Report::new(KernelError::Rejected)
            .attach_printable("Invalid characters in WebFinger domain"));
    }
    let webfinger_url = format!(
        "https://{}/.well-known/webfinger?resource=acct:{}@{}",
        domain, user, domain
    );
    let url = reqwest::Url::parse(&webfinger_url).map_err(|e| {
        Report::new(KernelError::Rejected).attach_printable(format!("Invalid WebFinger URL: {e}"))
    })?;
    let resolved_addresses = validate_fetch_url(&url).await?;
    let response = client_for_url(&url, &resolved_addresses)?
        .get(url)
        .header(ACCEPT, "application/jrd+json")
        .header(USER_AGENT, "Emumet/0.1 WebFinger resolver")
        .send()
        .await
        .map_err(|e| {
            Report::new(KernelError::Rejected)
                .attach_printable(format!("WebFinger request failed: {e}"))
        })?;
    if !response.status().is_success() {
        return Err(Report::new(KernelError::Rejected).attach_printable(format!(
            "WebFinger returned {} for {}@{}",
            response.status(),
            user,
            domain
        )));
    }
    let jrd: serde_json::Value = response.json().await.map_err(|e| {
        Report::new(KernelError::Rejected)
            .attach_printable(format!("WebFinger response is not valid JSON: {e}"))
    })?;
    let subject = jrd.get("subject").and_then(|s| s.as_str()).ok_or_else(|| {
        Report::new(KernelError::Rejected)
            .attach_printable("WebFinger response missing subject field")
    })?;
    let expected_subject = format!("acct:{}@{}", user, domain);
    if subject != expected_subject {
        return Err(Report::new(KernelError::Rejected).attach_printable(format!(
            "WebFinger subject mismatch: expected {expected_subject}, got {subject}"
        )));
    }
    let actor_url = jrd
        .get("links")
        .and_then(|links| links.as_array())
        .and_then(|links| {
            links.iter().find_map(|link| {
                let rel = link.get("rel").and_then(|r| r.as_str())?;
                let type_ = link.get("type").and_then(|t| t.as_str())?;
                let href = link.get("href").and_then(|h| h.as_str())?;
                if rel == "self"
                    && (type_ == "application/activity+json" || type_ == "application/ld+json")
                {
                    Some(href.to_string())
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| {
            Report::new(KernelError::NotFound).attach_printable(format!(
                "No ActivityPub link found in WebFinger response for {}@{}",
                user, domain
            ))
        })?;
    resolve_remote_actor(&actor_url).await
}

async fn upsert_remote_account<R, E>(
    repository: &R,
    executor: &mut E,
    actor: ResolvedRemoteActor,
) -> error_stack::Result<RemoteAccount, KernelError>
where
    R: RemoteAccountRepository<Executor = E>,
    E: kernel::interfaces::database::Executor,
{
    if let Some(existing) = repository.find_by_url(executor, &actor.url).await? {
        let updated = RemoteAccount::new(
            existing.id().clone(),
            actor.acct,
            actor.url,
            existing.icon_id().clone(),
            actor.inbox_url,
            actor.public_key_pem,
        );
        repository.update(executor, &updated).await?;
        return Ok(updated);
    }

    let remote_account = RemoteAccount::new(
        RemoteAccountId::new(kernel::generate_id()),
        actor.acct,
        actor.url,
        None,
        actor.inbox_url,
        actor.public_key_pem,
    );
    repository.create(executor, &remote_account).await?;
    Ok(remote_account)
}

async fn find_existing_follow<R, E>(
    repository: &R,
    executor: &mut E,
    source: &FollowTargetId,
    destination: &FollowTargetId,
) -> error_stack::Result<Option<Follow>, KernelError>
where
    R: FollowRepository<Executor = E>,
    E: kernel::interfaces::database::Executor,
{
    let followers = repository.find_followers(executor, destination).await?;
    Ok(followers
        .into_iter()
        .find(|follow| follow.source() == source && follow.destination() == destination))
}

async fn find_existing_following<R, E>(
    repository: &R,
    executor: &mut E,
    source: &FollowTargetId,
    destination: &FollowTargetId,
) -> error_stack::Result<Option<Follow>, KernelError>
where
    R: FollowRepository<Executor = E>,
    E: kernel::interfaces::database::Executor,
{
    let followings = repository.find_followings(executor, source).await?;
    Ok(followings
        .into_iter()
        .find(|follow| follow.source() == source && follow.destination() == destination))
}

fn undo_object_is_follow(activity: &Activity) -> bool {
    undo_follow_object(activity).is_some()
}

fn undo_follow_object(activity: &Activity) -> Option<Activity> {
    let object = activity.object.as_ref()?;
    serde_json::from_value::<Activity>(object.clone())
        .ok()
        .filter(|activity| activity.type_ == "Follow")
}

fn activity_object_id(activity: &Activity) -> Option<String> {
    match activity.object.as_ref()? {
        Value::String(value) => Some(value.clone()),
        Value::Object(map) => map.get("id").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

fn ensure_local_actor_matches(
    public_base_url: &PublicBaseUrl,
    account_nanoid: &str,
    object_id: &str,
) -> error_stack::Result<(), KernelError> {
    let expected = local_actor_url(public_base_url, account_nanoid);
    if object_id.trim_end_matches('/') == expected {
        Ok(())
    } else {
        Err(Report::new(KernelError::Rejected).attach_printable(format!(
            "Follow object does not match local actor: expected {expected}, got {object_id}"
        )))
    }
}

fn same_activitypub_id(left: &str, right: &str) -> bool {
    left.trim_end_matches('/') == right.trim_end_matches('/')
}

fn local_actor_url(public_base_url: &PublicBaseUrl, account_nanoid: &str) -> String {
    format!(
        "{}/accounts/{}",
        public_base_url.as_str().trim_end_matches('/'),
        account_nanoid
    )
}

fn accept_activity(
    public_base_url: &PublicBaseUrl,
    follow: &Follow,
    actor: &str,
    original_follow: Activity,
) -> error_stack::Result<Activity, KernelError> {
    // The Accept activity must be directed TO the follower (original Follow's actor),
    // not to the local actor who is sending the Accept.
    let remote_follower_url = &original_follow.actor;
    let object = serde_json::to_value(original_follow.clone()).map_err(|e| {
        Report::from(e)
            .change_context(KernelError::Internal)
            .attach_printable("Failed to serialize original Follow activity")
    })?;
    Ok(Activity {
        context: Some(Value::String(ACTIVITYSTREAMS_CONTEXT.to_string())),
        id: format!(
            "{}/activities/{}",
            public_base_url.as_str().trim_end_matches('/'),
            follow.id().as_ref()
        ),
        type_: "Accept".to_string(),
        actor: actor.to_string(),
        object: Some(object),
        target: None,
        to: Some(vec![remote_follower_url.to_string()]),
        cc: None,
    })
}

fn follow_activity(
    public_base_url: &PublicBaseUrl,
    follow: &Follow,
    local_actor_url: &str,
    remote_actor_url: &str,
) -> error_stack::Result<Activity, KernelError> {
    Ok(Activity {
        context: Some(Value::String(ACTIVITYSTREAMS_CONTEXT.to_string())),
        id: format!(
            "{}/activities/{}",
            public_base_url.as_str().trim_end_matches('/'),
            follow.id().as_ref()
        ),
        type_: "Follow".to_string(),
        actor: local_actor_url.to_string(),
        object: Some(serde_json::Value::String(remote_actor_url.to_string())),
        target: None,
        to: Some(vec![remote_actor_url.to_string()]),
        cc: None,
    })
}

fn host_header(url: &reqwest::Url) -> error_stack::Result<String, KernelError> {
    let host = url.host_str().ok_or_else(|| {
        Report::new(KernelError::Rejected).attach_printable("URL host is missing")
    })?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

async fn validate_fetch_url(
    url: &reqwest::Url,
) -> error_stack::Result<Vec<SocketAddr>, KernelError> {
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable(format!("SsrfBlocked: unsupported URL scheme '{scheme}'")));
        }
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(Report::new(KernelError::Rejected)
            .attach_printable("SsrfBlocked: URL credentials are not allowed"));
    }

    let host = url.host_str().ok_or_else(|| {
        Report::new(KernelError::Rejected).attach_printable("SsrfBlocked: URL host is empty")
    })?;
    let host_lc = host.trim_end_matches('.').to_ascii_lowercase();

    if cfg!(not(any(test, feature = "test-mode"))) {
        if !is_fetch_host_allowed(&host_lc)
            && (host_lc == "localhost" || host_lc.ends_with(".localhost"))
        {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("SsrfBlocked: localhost URL is not allowed"));
        }
    }

    let port = url.port_or_known_default().ok_or_else(|| {
        Report::new(KernelError::Rejected).attach_printable("SsrfBlocked: URL has no usable port")
    })?;
    if let Ok(ip) = host_lc.parse::<IpAddr>() {
        if cfg!(not(any(test, feature = "test-mode"))) {
            validate_public_ip(ip)?;
        }
        return Ok(vec![SocketAddr::new(ip, port)]);
    }

    let addresses = tokio::net::lookup_host((host_lc.as_str(), port))
        .await
        .map_err(|e| {
            Report::new(KernelError::Rejected)
                .attach_printable(format!("SsrfBlocked: DNS resolution failed: {e}"))
        })?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(Report::new(KernelError::Rejected)
            .attach_printable("SsrfBlocked: DNS resolution returned no addresses"));
    }
    if cfg!(not(any(test, feature = "test-mode"))) {
        for address in &addresses {
            validate_public_ip(address.ip())?;
        }
    }
    Ok(addresses)
}

fn client_for_url(
    url: &reqwest::Url,
    resolved_addresses: &[SocketAddr],
) -> error_stack::Result<reqwest::Client, KernelError> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10));
    #[cfg(any(test, feature = "test-mode"))]
    if std::env::var("AP_TEST_ACCEPT_INVALID_CERTS").as_deref() == Ok("1") {
        builder = builder.danger_accept_invalid_certs(true);
    }
    if let Some(host) = url.host_str() {
        if host.parse::<IpAddr>().is_err() {
            builder = builder.resolve_to_addrs(host, resolved_addresses);
        }
    }
    builder.build().map_err(|e| {
        Report::new(KernelError::Internal)
            .attach_printable(format!("Failed to build pinned HTTP client: {e}"))
    })
}

/// Checks whether a host is allowlisted for test-mode AP fetch operations.
///
/// Reads the `AP_TEST_ALLOWED_FETCH_HOSTS` environment variable (comma-separated,
/// trimmed, lowercase) and returns true if `host_lc` matches any entry.
/// Returns false when the env var is unset or empty.
fn is_fetch_host_allowed(host_lc: &str) -> bool {
    std::env::var("AP_TEST_ALLOWED_FETCH_HOSTS")
        .ok()
        .is_some_and(|val| {
            val.split(',')
                .any(|entry| entry.trim().eq_ignore_ascii_case(host_lc))
        })
}

async fn deliver_activity_to_inbox<D, S>(
    database_connection: &D,
    signing_key_repository: &S,
    password_provider: &impl PasswordProvider,
    key_encryptor: &impl KeyEncryptor,
    http_signer: &impl HttpSigner,
    account_id: &AccountId,
    inbox_url: &str,
    activity: &Activity,
    activity_name: &str,
) -> error_stack::Result<(), KernelError>
where
    D: DatabaseConnection,
    S: SigningKeyRepository<Executor = D::Executor>,
{
    let body = serde_json::to_vec(activity).map_err(|e| {
        Report::from(e)
            .change_context(KernelError::Internal)
            .attach_printable(format!("Failed to serialize {activity_name} activity"))
    })?;
    let url = reqwest::Url::parse(inbox_url).map_err(|e| {
        Report::new(KernelError::Rejected)
            .attach_printable(format!("Remote inbox URL is invalid: {e}"))
    })?;
    let resolved_addresses = validate_fetch_url(&url).await?;
    let host = host_header(&url)?;
    let digest = format!(
        "SHA-256={}",
        general_purpose::STANDARD.encode(sha2::Sha256::digest(&body))
    );
    let date = httpdate::fmt_http_date(std::time::SystemTime::now());
    let mut headers = std::collections::HashMap::new();
    headers.insert("host".to_string(), host.clone());
    headers.insert("date".to_string(), date.clone());
    headers.insert("digest".to_string(), digest.clone());
    headers.insert("content-type".to_string(), ACTIVITY_JSON.to_string());

    let signing_request = HttpSigningRequest {
        method: "POST".to_string(),
        url: inbox_url.to_string(),
        headers,
        body: Some(body.clone()),
    };
    let mut executor = database_connection.get_executor().await?;
    let signing_key = signing_key_repository
        .find_active_by_account_id(&mut executor, account_id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| {
            Report::new(KernelError::NotFound)
                .attach_printable("No active signing key found for account")
        })?;
    let password = password_provider.get_password()?;
    let private_key_pem = key_encryptor.decrypt(signing_key.encrypted_private_key(), &password)?;
    let signature = http_signer
        .sign(
            &signing_request,
            &private_key_pem,
            &signing_key.key_id_uri,
            signing_key.algorithm(),
        )
        .await?;

    let client = client_for_url(&url, &resolved_addresses)?;
    let mut request = client
        .post(url)
        .header(HOST, host)
        .header(DATE, date)
        .header("Digest", digest)
        .header(CONTENT_TYPE, ACTIVITY_JSON)
        .body(body);
    for (name, value) in &signature.cavage_headers {
        // Skip headers already set explicitly above to avoid duplicates.
        // nginx returns 400 for duplicate Host headers.
        let lower = name.to_ascii_lowercase();
        if lower == "host" || lower == "date" || lower == "digest" || lower == "content-type" {
            continue;
        }
        request = request.header(name.as_str(), value.as_str());
    }
    let response = request.send().await.map_err(|e| {
        Report::new(KernelError::Rejected)
            .attach_printable(format!("{activity_name} delivery failed: {e}"))
    })?;
    if !response.status().is_success() {
        return Err(Report::new(KernelError::Rejected).attach_printable(format!(
            "{activity_name} delivery returned {}",
            response.status()
        )));
    }
    Ok(())
}

#[cfg(not(test))]
fn validate_public_ip(ip: IpAddr) -> error_stack::Result<(), KernelError> {
    let blocked = match ip {
        IpAddr::V4(ip) => is_blocked_ipv4(ip),
        IpAddr::V6(ip) => is_blocked_ipv6(ip),
    };
    if blocked {
        Err(Report::new(KernelError::Rejected).attach_printable(format!(
            "SsrfBlocked: non-public IP address is not allowed: {ip}"
        )))
    } else {
        Ok(())
    }
}

#[cfg(not(test))]
fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || ip.is_unspecified()
        || octets[0] == 0
        || octets[0] >= 224
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 198 && (18..=19).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
}

#[cfg(not(test))]
fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(ipv4) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(ipv4);
    }

    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (ip.segments()[0] & 0xfe00) == 0xfc00
        || (ip.segments()[0] & 0xffc0) == 0xfe80
        || (ip.segments()[0] & 0xffff) == 0x2001 && (ip.segments()[1] & 0xfff0) == 0x0db8
        || ip.segments()[0] == 0x2002
        || (ip.segments()[0] == 0x2001 && ip.segments()[1] == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::interfaces::config::PublicBaseUrl;
    use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
    use kernel::prelude::entity::{AuthAccountId, Follow, FollowApprovedAt, FollowId};
    use kernel::test_utils::AccountBuilder;
    use std::sync::Mutex;
    use time::OffsetDateTime;

    #[derive(Clone)]
    struct MockExecutor;

    impl Executor for MockExecutor {}

    struct MockDatabaseConnection;

    impl DatabaseConnection for MockDatabaseConnection {
        type Executor = MockExecutor;

        async fn get_executor(&self) -> error_stack::Result<Self::Executor, KernelError> {
            Ok(MockExecutor)
        }
    }

    struct MockAccountQueryProcessor {
        account: Account,
    }

    impl AccountQueryProcessor for MockAccountQueryProcessor {
        type Executor = MockExecutor;

        async fn find_by_id(
            &self,
            _executor: &mut Self::Executor,
            id: &AccountId,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            Ok((self.account.id() == id).then(|| self.account.clone()))
        }

        async fn find_by_auth_id(
            &self,
            _executor: &mut Self::Executor,
            _auth_id: &AuthAccountId,
        ) -> error_stack::Result<Vec<Account>, KernelError> {
            Ok(Vec::new())
        }

        async fn find_by_name(
            &self,
            _executor: &mut Self::Executor,
            name: &AccountName,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            Ok((self.account.name() == name).then(|| self.account.clone()))
        }

        async fn find_by_nanoid(
            &self,
            _executor: &mut Self::Executor,
            nanoid: &Nanoid<Account>,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            Ok((self.account.nanoid() == nanoid).then(|| self.account.clone()))
        }

        async fn find_by_nanoids(
            &self,
            _executor: &mut Self::Executor,
            _nanoids: &[Nanoid<Account>],
        ) -> error_stack::Result<Vec<Account>, KernelError> {
            Ok(Vec::new())
        }

        async fn find_by_id_unfiltered(
            &self,
            executor: &mut Self::Executor,
            id: &AccountId,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            self.find_by_id(executor, id).await
        }

        async fn find_by_nanoid_unfiltered(
            &self,
            executor: &mut Self::Executor,
            nanoid: &Nanoid<Account>,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            self.find_by_nanoid(executor, nanoid).await
        }

        async fn find_by_nanoids_unfiltered(
            &self,
            executor: &mut Self::Executor,
            nanoids: &[Nanoid<Account>],
        ) -> error_stack::Result<Vec<Account>, KernelError> {
            self.find_by_nanoids(executor, nanoids).await
        }
    }

    struct MockFollowRepository {
        followers: Vec<Follow>,
        followings: Vec<Follow>,
    }

    impl FollowRepository for MockFollowRepository {
        type Executor = MockExecutor;

        async fn find_followings(
            &self,
            _executor: &mut Self::Executor,
            _source: &FollowTargetId,
        ) -> error_stack::Result<Vec<Follow>, KernelError> {
            Ok(self.followings.clone())
        }

        async fn find_followers(
            &self,
            _executor: &mut Self::Executor,
            _destination: &FollowTargetId,
        ) -> error_stack::Result<Vec<Follow>, KernelError> {
            Ok(self.followers.clone())
        }

        async fn create(
            &self,
            _executor: &mut Self::Executor,
            _follow: &Follow,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }

        async fn update(
            &self,
            _executor: &mut Self::Executor,
            _follow: &Follow,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }

        async fn delete(
            &self,
            _executor: &mut Self::Executor,
            _follow_id: &FollowId,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }
    }

    struct MockOutboxActivityRepository {
        activities: Mutex<Vec<OutboxActivity>>,
    }

    impl OutboxActivityRepository for MockOutboxActivityRepository {
        type Executor = MockExecutor;

        async fn create(
            &self,
            _executor: &mut Self::Executor,
            activity: &OutboxActivity,
        ) -> error_stack::Result<(), KernelError> {
            self.activities.lock().unwrap().push(activity.clone());
            Ok(())
        }

        async fn find_by_account_id(
            &self,
            _executor: &mut Self::Executor,
            account_id: &AccountId,
            limit: usize,
            cursor: Option<i64>,
        ) -> error_stack::Result<Vec<OutboxActivity>, KernelError> {
            let mut activities = self
                .activities
                .lock()
                .unwrap()
                .iter()
                .filter(|activity| &activity.account_id == account_id)
                .filter(|activity| cursor.map_or(true, |cursor| activity.id < cursor))
                .cloned()
                .collect::<Vec<_>>();
            activities.sort_by(|left, right| right.id.cmp(&left.id));
            activities.truncate(limit);
            Ok(activities)
        }

        async fn count_by_account_id(
            &self,
            _executor: &mut Self::Executor,
            account_id: &AccountId,
        ) -> error_stack::Result<u64, KernelError> {
            Ok(self
                .activities
                .lock()
                .unwrap()
                .iter()
                .filter(|activity| &activity.account_id == account_id)
                .count() as u64)
        }
    }

    struct MockModule {
        database: MockDatabaseConnection,
        accounts: MockAccountQueryProcessor,
        follows: MockFollowRepository,
        outbox: MockOutboxActivityRepository,
        public_base_url: PublicBaseUrl,
    }

    impl DependOnDatabaseConnection for MockModule {
        type DatabaseConnection = MockDatabaseConnection;

        fn database_connection(&self) -> &Self::DatabaseConnection {
            &self.database
        }
    }

    impl DependOnAccountQueryProcessor for MockModule {
        type AccountQueryProcessor = MockAccountQueryProcessor;

        fn account_query_processor(&self) -> &Self::AccountQueryProcessor {
            &self.accounts
        }
    }

    impl DependOnFollowRepository for MockModule {
        type FollowRepository = MockFollowRepository;

        fn follow_repository(&self) -> &Self::FollowRepository {
            &self.follows
        }
    }

    impl DependOnOutboxActivityRepository for MockModule {
        type OutboxActivityRepository = MockOutboxActivityRepository;

        fn outbox_activity_repository(&self) -> &Self::OutboxActivityRepository {
            &self.outbox
        }
    }

    impl DependOnPublicBaseUrl for MockModule {
        fn public_base_url(&self) -> &PublicBaseUrl {
            &self.public_base_url
        }
    }

    fn follow(source: AccountId, destination: AccountId, approved: bool) -> Follow {
        kernel::ensure_generator_initialized();
        Follow::new(
            FollowId::new(kernel::generate_id()),
            FollowTargetId::from(source),
            FollowTargetId::from(destination),
            approved.then(FollowApprovedAt::default),
        )
        .unwrap()
    }

    fn module() -> (MockModule, AccountId) {
        kernel::ensure_generator_initialized();
        let account_id = AccountId::default();
        let account = AccountBuilder::new()
            .id(account_id.clone())
            .name("alice")
            .nanoid(Nanoid::new("alice".to_string()))
            .build();
        let approved_follower = AccountId::default();
        let pending_follower = AccountId::default();
        let approved_followee = AccountId::default();
        let pending_followee = AccountId::default();

        (
            MockModule {
                database: MockDatabaseConnection,
                accounts: MockAccountQueryProcessor { account },
                follows: MockFollowRepository {
                    followers: vec![
                        follow(approved_follower, account_id.clone(), true),
                        follow(pending_follower, account_id.clone(), false),
                    ],
                    followings: vec![
                        follow(account_id.clone(), approved_followee, true),
                        follow(account_id.clone(), pending_followee, false),
                    ],
                },
                outbox: MockOutboxActivityRepository {
                    activities: Mutex::new(vec![outbox_activity(1, account_id.clone(), "Create")]),
                },
                public_base_url: PublicBaseUrl::new("https://example.com/".to_string()),
            },
            account_id,
        )
    }

    fn outbox_activity(id: i64, account_id: AccountId, activity_type: &str) -> OutboxActivity {
        let activity_id = format!("https://example.com/activities/{id}");
        OutboxActivity {
            id,
            account_id,
            activity_id: activity_id.clone(),
            activity_type: activity_type.to_string(),
            object_json: serde_json::json!({
                "@context": "https://www.w3.org/ns/activitystreams",
                "id": activity_id,
                "type": activity_type,
                "actor": "https://example.com/accounts/alice"
            })
            .to_string(),
            created_at: OffsetDateTime::now_utc(),
        }
    }

    fn follow_activity(actor: &str, object: &str) -> Activity {
        Activity {
            context: None,
            id: "https://remote.example/activities/follow-1".to_string(),
            type_: "Follow".to_string(),
            actor: actor.to_string(),
            object: Some(serde_json::Value::String(object.to_string())),
            target: None,
            to: None,
            cc: None,
        }
    }

    #[test]
    fn undo_object_is_follow_detects_nested_follow() {
        let follow = follow_activity(
            "https://remote.example/users/bob",
            "https://example.com/accounts/alice",
        );
        let undo = Activity {
            context: None,
            id: "https://remote.example/activities/undo-1".to_string(),
            type_: "Undo".to_string(),
            actor: "https://remote.example/users/bob".to_string(),
            object: Some(serde_json::to_value(follow).unwrap()),
            target: None,
            to: None,
            cc: None,
        };

        assert!(undo_object_is_follow(&undo));
    }

    #[test]
    fn accept_activity_wraps_original_follow() {
        let (module, account_id) = module();
        let follow = follow(AccountId::default(), account_id, true);
        let original = follow_activity(
            "https://remote.example/users/bob",
            "https://example.com/accounts/alice",
        );

        let accept = accept_activity(
            module.public_base_url(),
            &follow,
            "https://example.com/accounts/alice",
            original,
        )
        .unwrap();

        assert_eq!(accept.type_, "Accept");
        assert_eq!(accept.actor, "https://example.com/accounts/alice");
        assert_eq!(
            accept.id,
            format!("https://example.com/activities/{}", follow.id().as_ref())
        );
        assert_eq!(
            accept.object.as_ref().and_then(|value| value.get("type")),
            Some(&serde_json::Value::String("Follow".to_string()))
        );
        // Accept must be directed TO the follower (original Follow's actor),
        // not to the local actor who is sending the Accept.
        assert_eq!(
            accept.to,
            Some(vec!["https://remote.example/users/bob".to_string()]),
            "Accept.to should target the remote follower, not the local actor"
        );
    }

    #[test]
    fn local_actor_match_rejects_wrong_follow_object() {
        let public_base_url = PublicBaseUrl::new("https://example.com/".to_string());

        assert!(ensure_local_actor_matches(
            &public_base_url,
            "alice",
            "https://example.com/accounts/alice/"
        )
        .is_ok());
        assert!(ensure_local_actor_matches(
            &public_base_url,
            "alice",
            "https://example.com/accounts/bob"
        )
        .is_err());
    }

    #[tokio::test]
    async fn followers_collection_returns_ordered_collection_structure() {
        let (module, account_id) = module();

        let collection = module.get_followers_collection(&account_id).await.unwrap();

        assert_eq!(
            collection.id,
            "https://example.com/accounts/alice/followers"
        );
        assert_eq!(collection.type_, "OrderedCollection");
        assert_eq!(collection.total_items, Some(1));
        assert_eq!(collection.first, None);
        assert_eq!(collection.last, None);
    }

    #[tokio::test]
    async fn following_collection_returns_ordered_collection_structure() {
        let (module, account_id) = module();

        let collection = module.get_following_collection(&account_id).await.unwrap();

        assert_eq!(
            collection.id,
            "https://example.com/accounts/alice/following"
        );
        assert_eq!(collection.type_, "OrderedCollection");
        assert_eq!(collection.total_items, Some(1));
        assert_eq!(collection.first, None);
        assert_eq!(collection.last, None);
    }

    #[tokio::test]
    async fn store_outbox_activity_persists_activity() {
        let (module, account_id) = module();
        let activity = outbox_activity(2, account_id.clone(), "Accept");

        module.store_outbox_activity(&activity).await.unwrap();

        let mut executor = MockExecutor;
        let activities = module
            .outbox_activity_repository()
            .find_by_account_id(&mut executor, &account_id, 10, None)
            .await
            .unwrap();
        assert!(activities.iter().any(|stored| stored.id == 2));
    }

    #[tokio::test]
    async fn outbox_collection_returns_ordered_collection_with_items() {
        let (module, account_id) = module();

        let collection = module
            .get_outbox_collection(&account_id, 10, None)
            .await
            .unwrap();

        assert_eq!(collection.id, "https://example.com/accounts/alice/outbox");
        assert_eq!(collection.type_, "OrderedCollection");
        assert_eq!(collection.total_items, Some(1));
        assert_eq!(collection.ordered_items.as_ref().unwrap().len(), 1);
        assert_eq!(collection.ordered_items.unwrap()[0]["type"], "Create");
    }

    #[tokio::test]
    async fn empty_outbox_collection_returns_zero_items() {
        let (module, account_id) = module();
        module.outbox.activities.lock().unwrap().clear();

        let collection = module
            .get_outbox_collection(&account_id, 10, None)
            .await
            .unwrap();

        assert_eq!(collection.total_items, Some(0));
        assert!(collection.ordered_items.unwrap().is_empty());
    }
}
