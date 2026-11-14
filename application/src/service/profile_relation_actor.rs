//! 個別 block・mute の対象 actor の表示情報 (D08 ui-spec 1-6)。
//!
//! local の対象は Profile から直接、remote の対象は永続キャッシュ (`profile_remote_actor_cache`) から
//! 返す。**表示情報の取得 (リモートへの接続) は一覧・作成の処理中には行わない**: 一覧は未取得・期限切れの
//! 対象を `refresh` として返し、呼び出し側が応答本体を取り出し終えた後に [`ProfileRelationActorUseCase::refresh_remote_actor_profiles`]
//! を (バックグラウンドで) 呼ぶ。取得の前に DB のリース (`try_claim_refresh`) を原子的に確保するので、同じ
//! actor を複数のリクエスト・プロセスが同時に要求しても取得は 1 回。

use crate::dto::profile_relationship::{RelationActorDto, RelationActorKind, RelationActorStatus};
use crate::service::profile_relationship::activity::local_profile_nanoid;
use crate::service::profile_relationship::Conn;
use kernel::activitypub::ActorUrlBuilder;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::database::{DependOnTransactionManager, TransactionManager};
use kernel::interfaces::read_model::{
    DependOnProfileReadModel, ProfileProjection, ProfileReadModel,
};
use kernel::interfaces::repository::{
    DependOnImageRepository, DependOnProfileRemoteActorCacheRepository,
    DependOnRemoteActorResolver, ImageRepository, ProfileRemoteActorCacheRepository,
    RemoteActorCacheEntry, RemoteActorCacheStatus, RemoteActorResolver,
};
use kernel::prelude::entity::{GraphActorUrl, ImageId, Nanoid};
use kernel::KernelError;
use std::future::Future;
use time::{Duration, OffsetDateTime};

/// 1 回の一覧応答で取得を予約する remote actor の最大数。
pub const REMOTE_ACTOR_REFRESH_BATCH: usize = 10;
/// 取得中のリース (この間は他の取得を始めない。取得が異常終了してもこの後に再取得できる)。
pub const REMOTE_ACTOR_REFRESH_LEASE: Duration = Duration::seconds(60);
/// 取得に成功した情報を再取得するまでの期間。
pub const REMOTE_ACTOR_REFRESH_TTL: Duration = Duration::hours(24);
/// 取得に失敗した actor を再試行するまでの期間。
pub const REMOTE_ACTOR_RETRY_AFTER: Duration = Duration::minutes(15);

/// 表示情報を求める対象。
pub(crate) enum ActorToDescribe {
    Local(ProfileProjection),
    Remote(GraphActorUrl),
}

/// `host` または `host:port` (URL の既定ポートは省略、host は小文字)。
fn host_of(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

fn unresolved(
    kind: RelationActorKind,
    actor_url: String,
    profile_nanoid: Option<String>,
    status: RelationActorStatus,
) -> RelationActorDto {
    RelationActorDto {
        kind,
        actor_url,
        profile_nanoid,
        status,
        acct: None,
        preferred_username: None,
        display_name: None,
        icon_url: None,
    }
}

fn remote_dto(url: &GraphActorUrl, entry: Option<&RemoteActorCacheEntry>) -> RelationActorDto {
    let actor_url = url.as_str().to_string();
    let status = match entry.map(|entry| entry.status) {
        Some(RemoteActorCacheStatus::Resolved) => RelationActorStatus::Resolved,
        Some(RemoteActorCacheStatus::Failed) => RelationActorStatus::Unavailable,
        Some(RemoteActorCacheStatus::Pending) | None => RelationActorStatus::Pending,
    };
    let profile = entry.and_then(|entry| entry.profile.as_ref());
    match (status, profile, host_of(&actor_url)) {
        (RelationActorStatus::Resolved, Some(profile), Some(host)) => RelationActorDto {
            kind: RelationActorKind::Remote,
            actor_url,
            profile_nanoid: None,
            status,
            acct: Some(format!("{}@{host}", profile.preferred_username())),
            preferred_username: Some(profile.preferred_username().to_string()),
            display_name: profile.display_name().map(str::to_string),
            icon_url: profile.icon_url().map(str::to_string),
        },
        (RelationActorStatus::Resolved, _, _) => unresolved(
            RelationActorKind::Remote,
            actor_url,
            None,
            RelationActorStatus::Unavailable,
        ),
        _ => unresolved(RelationActorKind::Remote, actor_url, None, status),
    }
}

/// 対象ごとの表示情報 (入力と同じ順) と、取得を予約する remote actor (キャッシュ行が無い・期限切れ。最大
/// `REMOTE_ACTOR_REFRESH_BATCH` 件)。リモートへは接続しない。
pub(crate) async fn describe_actors<D>(
    deps: &D,
    executor: &mut Conn<D>,
    targets: Vec<ActorToDescribe>,
    now: OffsetDateTime,
) -> error_stack::Result<(Vec<RelationActorDto>, Vec<GraphActorUrl>), KernelError>
where
    D: DependOnTransactionManager
        + DependOnProfileReadModel
        + DependOnImageRepository
        + DependOnProfileRemoteActorCacheRepository
        + DependOnPublicBaseUrl,
{
    let image_ids: Vec<ImageId> = targets
        .iter()
        .filter_map(|target| match target {
            ActorToDescribe::Local(profile) => profile.icon().clone(),
            ActorToDescribe::Remote(_) => None,
        })
        .collect();
    let images = if image_ids.is_empty() {
        Vec::new()
    } else {
        deps.image_repository()
            .find_by_ids(executor, &image_ids)
            .await?
    };
    let remote_urls: Vec<GraphActorUrl> = targets
        .iter()
        .filter_map(|target| match target {
            ActorToDescribe::Remote(url) => Some(url.clone()),
            ActorToDescribe::Local(_) => None,
        })
        .collect();
    let entries = deps
        .profile_remote_actor_cache_repository()
        .find_many(executor, &remote_urls)
        .await?;

    let base = deps.public_base_url().as_str().to_string();
    let local_host = host_of(&base);
    let mut refresh: Vec<GraphActorUrl> = Vec::new();
    let mut described = Vec::with_capacity(targets.len());
    for target in targets {
        match target {
            ActorToDescribe::Local(profile) => {
                let nanoid = profile.nanoid().as_ref().to_string();
                let username = profile
                    .username()
                    .as_ref()
                    .map(|username| username.as_ref().to_string());
                let icon_url = profile.icon().as_ref().and_then(|id| {
                    images
                        .iter()
                        .find(|image| image.id() == id)
                        .map(|image| image.url().as_ref().to_string())
                });
                described.push(RelationActorDto {
                    kind: RelationActorKind::Local,
                    actor_url: ActorUrlBuilder::for_profile(&base, &nanoid).actor_id(),
                    profile_nanoid: Some(nanoid),
                    status: RelationActorStatus::Resolved,
                    acct: username
                        .as_ref()
                        .zip(local_host.as_ref())
                        .map(|(username, host)| format!("{username}@{host}")),
                    preferred_username: username,
                    display_name: profile
                        .display_name()
                        .as_ref()
                        .map(|name| name.as_ref().to_string()),
                    icon_url,
                });
            }
            ActorToDescribe::Remote(url) => {
                let entry = entries.iter().find(|entry| entry.actor_url == url);
                let due = entry.is_none_or(|entry| entry.next_refresh_at <= now);
                if due && refresh.len() < REMOTE_ACTOR_REFRESH_BATCH && !refresh.contains(&url) {
                    refresh.push(url.clone());
                }
                described.push(remote_dto(&url, entry));
            }
        }
    }
    Ok((described, refresh))
}

pub trait ProfileRelationActorUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnTransactionManager
    + DependOnProfileReadModel
    + DependOnImageRepository
    + DependOnProfileRemoteActorCacheRepository
    + DependOnRemoteActorResolver
    + DependOnPublicBaseUrl
{
    /// actor URL (block / mute の `target`) の表示情報と、取得を予約する remote actor の URL。作成直後の応答に
    /// 使う。リモートへは接続しない。DB の読み取りに失敗したときは (作成済みの関係を失敗にしないため) 取得状態
    /// `Pending` の表示情報なしで返す。
    fn describe_relation_targets<'a>(
        &'a self,
        actor_urls: &'a [String],
    ) -> impl Future<Output = (Vec<RelationActorDto>, Vec<String>)> + Send + 'a {
        async move {
            let deps = self.clone();
            let owned_urls = actor_urls.to_vec();
            let described = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let mut targets = Vec::with_capacity(owned_urls.len());
                        let mut fallback = Vec::new();
                        for (index, raw) in owned_urls.iter().enumerate() {
                            if let Some(nanoid) = local_profile_nanoid(deps.public_base_url(), raw)
                            {
                                match deps
                                    .profile_read_model()
                                    .find_by_nanoid(executor, &Nanoid::new(nanoid.to_string()))
                                    .await?
                                {
                                    Some(profile) => targets.push(ActorToDescribe::Local(profile)),
                                    None => fallback.push((
                                        index,
                                        unresolved(
                                            RelationActorKind::Local,
                                            raw.clone(),
                                            Some(nanoid.to_string()),
                                            RelationActorStatus::Unavailable,
                                        ),
                                    )),
                                }
                            } else {
                                match GraphActorUrl::new(raw.as_str()) {
                                    Ok(url) => targets.push(ActorToDescribe::Remote(url)),
                                    Err(_) => fallback.push((
                                        index,
                                        unresolved(
                                            RelationActorKind::Remote,
                                            raw.clone(),
                                            None,
                                            RelationActorStatus::Unavailable,
                                        ),
                                    )),
                                }
                            }
                        }
                        let (mut described, refresh) =
                            describe_actors(&deps, executor, targets, OffsetDateTime::now_utc())
                                .await?;
                        for (index, dto) in fallback {
                            described.insert(index, dto);
                        }
                        Ok((
                            described,
                            refresh
                                .into_iter()
                                .map(|url| url.as_str().to_string())
                                .collect(),
                        ))
                    })
                })
                .await;
            match described {
                Ok(result) => result,
                Err(error) => {
                    // The relation itself was already stored; a failed display lookup must not turn
                    // that into an error response. The caller falls back to the URL.
                    tracing::warn!(error = ?error, "relation target display lookup failed");
                    let fallback = actor_urls
                        .iter()
                        .map(|raw| {
                            let local = local_profile_nanoid(self.public_base_url(), raw);
                            unresolved(
                                if local.is_some() {
                                    RelationActorKind::Local
                                } else {
                                    RelationActorKind::Remote
                                },
                                raw.clone(),
                                local.map(str::to_string),
                                RelationActorStatus::Pending,
                            )
                        })
                        .collect();
                    (fallback, Vec::new())
                }
            }
        }
    }

    /// remote actor の表示情報を取得してキャッシュを更新する (best-effort、応答本体を取り出し終えた後に呼ぶ)。各 actor は
    /// DB のリースを確保できたときだけ取得し、失敗は記録するだけで呼び出し元に返さない。
    fn refresh_remote_actor_profiles<'a>(
        &'a self,
        actor_urls: &'a [String],
    ) -> impl Future<Output = ()> + Send + 'a {
        self.refresh_remote_actor_profiles_with_clock(actor_urls, &OffsetDateTime::now_utc)
    }

    /// [`Self::refresh_remote_actor_profiles`] の時刻固定版 (期限の検査用)。
    fn refresh_remote_actor_profiles_at<'a>(
        &'a self,
        actor_urls: &'a [String],
        now: OffsetDateTime,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            self.refresh_remote_actor_profiles_with_clock(actor_urls, &|| now)
                .await
        }
    }

    /// [`Self::refresh_remote_actor_profiles`] の時計指定版。時刻は actor ごとにリースの確保の直前と
    /// 結果の記録の直前に読む (先行 actor の取得に時間がかかっても、後続のリース・TTL・バックオフが
    /// バッチ開始時の時刻を基準に短くならない)。
    fn refresh_remote_actor_profiles_with_clock<'a>(
        &'a self,
        actor_urls: &'a [String],
        clock: &'a (dyn Fn() -> OffsetDateTime + Sync),
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            for raw in actor_urls {
                let Ok(url) = GraphActorUrl::new(raw.as_str()) else {
                    continue;
                };
                if let Err(error) = self.refresh_one(&url, clock).await {
                    tracing::warn!(
                        actor_url = %url.as_str(),
                        error = ?error,
                        "remote actor profile refresh could not be recorded"
                    );
                }
            }
        }
    }

    #[doc(hidden)]
    fn refresh_one<'a>(
        &'a self,
        url: &'a GraphActorUrl,
        clock: &'a (dyn Fn() -> OffsetDateTime + Sync),
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            let claim_at = clock();
            let deps = self.clone();
            let claim_url = url.clone();
            let claimed = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        deps.profile_remote_actor_cache_repository()
                            .try_claim_refresh(
                                executor,
                                &claim_url,
                                claim_at,
                                claim_at + REMOTE_ACTOR_REFRESH_LEASE,
                            )
                            .await
                    })
                })
                .await?;
            if !claimed {
                return Ok(());
            }
            // The fetch runs outside any transaction.
            let fetched = self.remote_actor_resolver().fetch_profile(url).await;
            let now = clock();
            let deps = self.clone();
            let record_url = url.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let repository = deps.profile_remote_actor_cache_repository();
                        match fetched {
                            Ok(profile) => {
                                repository
                                    .record_resolved(
                                        executor,
                                        &record_url,
                                        &profile,
                                        now,
                                        now + REMOTE_ACTOR_REFRESH_TTL,
                                    )
                                    .await
                            }
                            Err(error) => {
                                tracing::debug!(
                                    actor_url = %record_url.as_str(),
                                    error = ?error.current_context(),
                                    "remote actor profile fetch failed"
                                );
                                repository
                                    .record_failed(
                                        executor,
                                        &record_url,
                                        now + REMOTE_ACTOR_RETRY_AFTER,
                                    )
                                    .await
                            }
                        }
                    })
                })
                .await
        }
    }
}

impl<T> ProfileRelationActorUseCase for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnTransactionManager
        + DependOnProfileReadModel
        + DependOnImageRepository
        + DependOnProfileRemoteActorCacheRepository
        + DependOnRemoteActorResolver
        + DependOnPublicBaseUrl
{
}
