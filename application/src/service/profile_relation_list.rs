//! Owner-only reads on a Profile: the identity document and the individual block / mute lists
//! (E8 contract paths, mounted by the P4 cutover).
//!
//! Authorization is the same as for every other Profile operation: only the Profile's current
//! owner (`require_profile_owner`, which also share-locks the Profile row so a concurrent transfer
//! is serialized with the read). Settings of other Profiles and Accounts are never returned.

use crate::dto::profile::ProfileDto;
use crate::dto::profile_relationship::{ProfileRelationDto, ProfileRelationItemDto};
use crate::service::domain_control::require_profile_owner;
use crate::service::profile_relation_actor::{describe_actors, ActorToDescribe};
use crate::service::session_context::OrganizationContext;
use error_stack::Report;
use kernel::activitypub::ActorUrlBuilder;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::database::{DependOnTransactionManager, TransactionManager};
use kernel::interfaces::read_model::{
    AccountQuery, DependOnAccountQuery, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{
    DependOnDomainControlRepository, DependOnImageRepository, DependOnProfileBlockRepository,
    DependOnProfileMuteRepository, DependOnProfileRemoteActorCacheRepository, ImageRepository,
    ProfileBlockRepository, ProfileMuteRepository,
};
use kernel::prelude::entity::{AuthAccountId, ProfileBlockId, ProfileGraphActor, ProfileMuteId};
use kernel::KernelError;
use std::future::Future;

pub const DEFAULT_PAGE_SIZE: u32 = 20;
pub const MAX_PAGE_SIZE: u32 = 100;

/// One page of individual block / mute settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRelationPageDto {
    pub items: Vec<ProfileRelationItemDto>,
    pub next_cursor: Option<String>,
    /// 表示情報が未取得・期限切れの remote actor の URL (最大 `REMOTE_ACTOR_REFRESH_BATCH` 件)。応答を返した後に
    /// `ProfileRelationActorUseCase::refresh_remote_actor_profiles` へ渡す (一覧の処理中にはリモートへ接続しない)。
    pub refresh: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileRelationListKind {
    Block,
    Mute,
}

fn parse_cursor(cursor: Option<&str>) -> error_stack::Result<Option<i64>, KernelError> {
    cursor
        .map(|raw| {
            raw.parse::<i64>().ok().filter(|id| *id > 0).ok_or_else(|| {
                Report::new(KernelError::Validation)
                    .attach_printable("cursor is not a valid list cursor".to_string())
            })
        })
        .transpose()
}

pub trait ProfileRelationListUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnDomainControlRepository
    + DependOnProfileBlockRepository
    + DependOnProfileMuteRepository
    + DependOnProfileReadModel
    + DependOnImageRepository
    + DependOnProfileRemoteActorCacheRepository
    + DependOnTransactionManager
    + DependOnPublicBaseUrl
{
    /// Individual block or mute settings owned by the Profile, oldest first. `limit` is clamped
    /// to `1..=MAX_PAGE_SIZE`; `cursor` is the `next_cursor` of the previous page.
    fn list_profile_relations<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: &'a str,
        kind: ProfileRelationListKind,
        cursor: Option<&'a str>,
        limit: Option<u32>,
    ) -> impl Future<Output = error_stack::Result<ProfileRelationPageDto, KernelError>> + Send + 'a
    {
        async move {
            let after = parse_cursor(cursor)?;
            let limit = limit.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
            let deps = self.clone();
            let auth_account_id = auth_account_id.clone();
            let org_context = org_context.cloned();
            let profile_nanoid = profile_nanoid.to_string();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let profile_id = require_profile_owner(
                            &deps,
                            executor,
                            &auth_account_id,
                            org_context.as_ref(),
                            &profile_nanoid,
                        )
                        .await?;
                        // One extra row tells whether another page follows.
                        let fetch = i64::from(limit) + 1;
                        let mut rows: Vec<(i64, String, ProfileGraphActor, time::OffsetDateTime)> =
                            match kind {
                                ProfileRelationListKind::Block => deps
                                    .profile_block_repository()
                                    .list(
                                        executor,
                                        &profile_id,
                                        after.map(ProfileBlockId::new).as_ref(),
                                        fetch,
                                    )
                                    .await?
                                    .into_iter()
                                    .map(|row| {
                                        (
                                            *row.id().as_ref(),
                                            row.nanoid().as_ref().to_string(),
                                            row.target().clone(),
                                            *row.created_at(),
                                        )
                                    })
                                    .collect(),
                                ProfileRelationListKind::Mute => deps
                                    .profile_mute_repository()
                                    .list(
                                        executor,
                                        &profile_id,
                                        after.map(ProfileMuteId::new).as_ref(),
                                        fetch,
                                    )
                                    .await?
                                    .into_iter()
                                    .map(|row| {
                                        (
                                            *row.id().as_ref(),
                                            row.nanoid().as_ref().to_string(),
                                            row.target().clone(),
                                            *row.created_at(),
                                        )
                                    })
                                    .collect(),
                            };
                        let has_more = rows.len() > limit as usize;
                        rows.truncate(limit as usize);
                        let next_cursor = has_more
                            .then(|| rows.last().map(|row| row.0.to_string()))
                            .flatten();

                        let base = deps.public_base_url().as_str().to_string();
                        let mut relations = Vec::with_capacity(rows.len());
                        let mut targets = Vec::with_capacity(rows.len());
                        for (_, nanoid, target, created_at) in rows {
                            let (url, to_describe) = match target {
                                ProfileGraphActor::Remote(url) => {
                                    (url.as_str().to_string(), ActorToDescribe::Remote(url))
                                }
                                ProfileGraphActor::Local(local_id) => {
                                    let Some(local) = deps
                                        .profile_read_model()
                                        .find_by_id_unfiltered(executor, &local_id)
                                        .await?
                                    else {
                                        continue;
                                    };
                                    (
                                        ActorUrlBuilder::for_profile(
                                            &base,
                                            local.nanoid().as_ref(),
                                        )
                                        .actor_id(),
                                        ActorToDescribe::Local(local),
                                    )
                                }
                            };
                            relations.push(ProfileRelationDto {
                                nanoid,
                                profile_nanoid: profile_nanoid.clone(),
                                target: url,
                                created_at,
                            });
                            targets.push(to_describe);
                        }
                        // Display data comes from the Profile (local) or the cache (remote); this never
                        // contacts a remote server.
                        let (actors, refresh) = describe_actors(
                            &deps,
                            executor,
                            targets,
                            time::OffsetDateTime::now_utc(),
                        )
                        .await?;
                        let items = relations
                            .into_iter()
                            .zip(actors)
                            .map(|(relation, target_actor)| ProfileRelationItemDto {
                                relation,
                                target_actor,
                            })
                            .collect();
                        Ok(ProfileRelationPageDto {
                            items,
                            next_cursor,
                            refresh: refresh
                                .into_iter()
                                .map(|url| url.as_str().to_string())
                                .collect(),
                        })
                    })
                })
                .await
        }
    }

    /// The Profile identity (E8 `GET /api/v1/profiles/{profile_nanoid}`), owner only. `username`
    /// is `None` only for a Profile created before the cutover that has not set one (the cutover
    /// refuses to run while any exists, so this is a transitional value).
    fn get_profile_identity<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: &'a str,
    ) -> impl Future<Output = error_stack::Result<ProfileDto, KernelError>> + Send + 'a {
        async move {
            let deps = self.clone();
            let auth_account_id = auth_account_id.clone();
            let org_context = org_context.cloned();
            let profile_nanoid = profile_nanoid.to_string();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let profile_id = require_profile_owner(
                            &deps,
                            executor,
                            &auth_account_id,
                            org_context.as_ref(),
                            &profile_nanoid,
                        )
                        .await?;
                        let profile = deps
                            .profile_read_model()
                            .find_by_id_unfiltered(executor, &profile_id)
                            .await?
                            .ok_or_else(|| {
                                Report::new(KernelError::NotFound).attach_printable(format!(
                                    "Profile not found: {profile_nanoid}"
                                ))
                            })?;
                        let account = deps
                            .account_query()
                            .find_by_id(executor, profile.account_id())
                            .await?
                            .ok_or_else(|| {
                                Report::new(KernelError::NotFound)
                                    .attach_printable("Owning account not found")
                            })?;
                        let image_ids: Vec<_> = profile
                            .icon()
                            .iter()
                            .chain(profile.banner().iter())
                            .cloned()
                            .collect();
                        let images = deps
                            .image_repository()
                            .find_by_ids(executor, &image_ids)
                            .await?;
                        let url_of = |id: &Option<kernel::prelude::entity::ImageId>| {
                            id.as_ref().and_then(|id| {
                                images
                                    .iter()
                                    .find(|image| image.id() == id)
                                    .map(|image| image.url().as_ref().to_string())
                            })
                        };
                        let icon_url = url_of(profile.icon());
                        let banner_url = url_of(profile.banner());
                        Ok(ProfileDto::new(
                            profile,
                            account.nanoid().as_ref().to_string(),
                            icon_url,
                            banner_url,
                        ))
                    })
                })
                .await
        }
    }
}

impl<T> ProfileRelationListUseCase for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnAccountQuery
        + DependOnDomainControlRepository
        + DependOnProfileBlockRepository
        + DependOnProfileMuteRepository
        + DependOnProfileReadModel
        + DependOnImageRepository
        + DependOnProfileRemoteActorCacheRepository
        + DependOnTransactionManager
        + DependOnPublicBaseUrl
{
}
