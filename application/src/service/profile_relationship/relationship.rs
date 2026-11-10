//! Profile 単位の Follow / unfollow と個別 actor の block / mute の送信側 use case (P3o)。
//!
//! - すべての操作は現在の所有者だけが行える (`domain_control::require_profile_owner`。Profile 行を共有ロック
//!   して認可するので、移管と直列化される)。関係の主体は常に `Local(ProfileId)` で、同じ Account の別
//!   Profile や他 Account の Profile の関係には触れない。
//! - **関係の変更と配送 job の予約は同じ DB transaction** で行う (P2 の `ProfileDeliveryEnqueuer` に
//!   transaction 接続を渡す)。予約に失敗すれば関係の変更も残らず、関係の変更に失敗すれば job も残らない。
//! - **関係状態と配送状態は別物**: 関係 (ローカルの真実) は配送 job の成否に依存しない。外部配送が失敗・
//!   遅延しても unfollow / block は巻き戻らず、配送は worker (`delivery`) が再試行する。
//! - 個別 actor block は従来意味 (双方向 Follow 解除 + 以後の Follow 拒否) を Profile 単位で踏襲する。
//!   **domain 単位の方針は Follow の拒否にだけ使う (D8G)**: `follow_profile` が `domain_follow_guard` の
//!   判定 (検証済み canonical actor URL の host の完全一致・mode=block のみ) を関係ロック下で行い、
//!   mute は無視する。配送前の再判定は `delivery`。個別 block の適用 (双方向解除) は domain 方針と混ぜない。
//!   AP `Block` は送らず
//!   (D08)、解除された Follow を相手に伝える `Undo(Follow)` / `Reject(Follow)` だけを配送する。
//!   block を解除しても解除済みの関係は復元せず、何も配送しない。
//! - mute は表示抑制の設定だけで、既存の関係も配送も変えない。

use super::activity::{
    follow_activity_id, follow_body, local_profile_nanoid, profile_actor_url, reject_activity_id,
    reject_follow_body, undo_activity_id, undo_follow_body,
};
use crate::dto::profile_relationship::{ProfileFollowDto, ProfileRelationDto};
use crate::service::domain_control::require_profile_owner;
use crate::service::domain_follow_guard::domain_blocks_actor;
use crate::service::session_context::OrganizationContext;
use error_stack::Report;
use kernel::interfaces::config::{DependOnPublicBaseUrl, PublicBaseUrl};
use kernel::interfaces::database::{
    DatabaseConnection, DependOnDatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::read_model::{
    DependOnAccountQuery, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{
    AcctHandle, BeginFollowOutcome, DependOnDomainControlRepository,
    DependOnProfileBlockRepository, DependOnProfileDeliveryEnqueuer,
    DependOnProfileDeliveryJobRepository, DependOnProfileFollowRepository,
    DependOnProfileMuteRepository, DependOnProfileRelationshipLock, DependOnRemoteActorResolver,
    ProfileBlockRepository, ProfileDeliveryEnqueuer, ProfileDeliveryJobRepository,
    ProfileFollowRepository, ProfileMuteRepository, ProfileRelationshipLock, RemoteActorResolver,
};
use kernel::prelude::entity::{
    AuthAccountId, DeliveryActivityType, GraphActorUrl, Nanoid, NewProfileDeliveryJob,
    ProfileBlock, ProfileBlockId, ProfileDeliveryJobState, ProfileFollow, ProfileFollowEndReason,
    ProfileFollowId, ProfileGraphActor, ProfileId, ProfileMute, ProfileMuteId,
};
use kernel::KernelError;
use std::future::Future;
use time::{Duration, OffsetDateTime};

type Conn<T> =
    <<T as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection;

/// 送信中だった Follow job を取り消したとき、その Follow が相手に届く可能性のある時間。Undo はこの後に送る
/// (先に Undo が届き、後から Follow が届いて関係が残る逆転を避ける)。worker の既定 lease と同じ長さ。
const IN_FLIGHT_GRACE: Duration = Duration::seconds(120);

/// PostgreSQL の timestamptz に合わせてマイクロ秒へ丸める。
fn now() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    now.replace_nanosecond(now.nanosecond() / 1000 * 1000)
        .unwrap_or(now)
}

fn validation(message: &str) -> Report<KernelError> {
    Report::new(KernelError::Validation).attach_printable(message.to_string())
}

fn conflict(message: &str) -> Report<KernelError> {
    Report::new(KernelError::Concurrency).attach_printable(message.to_string())
}

fn not_found(message: &str) -> Report<KernelError> {
    Report::new(KernelError::NotFound).attach_printable(message.to_string())
}

/// 関係 use case が使う依存の束。
pub trait RelationshipDeps:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnDomainControlRepository
    + DependOnTransactionManager
    + DependOnProfileFollowRepository
    + DependOnProfileBlockRepository
    + DependOnProfileMuteRepository
    + DependOnProfileDeliveryEnqueuer
    + DependOnProfileDeliveryJobRepository
    + DependOnProfileRelationshipLock
    + DependOnProfileReadModel
    + DependOnPublicBaseUrl
{
}

impl<T> RelationshipDeps for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnAccountQuery
        + DependOnDomainControlRepository
        + DependOnTransactionManager
        + DependOnProfileFollowRepository
        + DependOnProfileBlockRepository
        + DependOnProfileMuteRepository
        + DependOnProfileDeliveryEnqueuer
        + DependOnProfileDeliveryJobRepository
        + DependOnProfileRelationshipLock
        + DependOnProfileReadModel
        + DependOnPublicBaseUrl
{
}

/// 要求された target の種別 (副作用なしで判定できる範囲)。
#[derive(Debug, Clone, PartialEq, Eq)]
enum TargetKind {
    /// 同じ instance の Profile (`{PUBLIC_BASE_URL}/ap/profiles/{nanoid}`)。
    Local(String),
    Remote(GraphActorUrl),
    /// `acct:name@domain`。transaction の外で WebFinger により actor URL へ解決する。
    Acct(AcctHandle),
}

fn classify_target(
    public_base_url: &PublicBaseUrl,
    raw: &str,
) -> error_stack::Result<TargetKind, KernelError> {
    // A value that is not a URL is an `acct:name@domain` handle (resolved over WebFinger) or a local
    // Profile nanoid.
    if !raw.contains("://") {
        if raw.starts_with("acct:") || raw.contains('@') {
            return Ok(TargetKind::Acct(AcctHandle::parse(raw)?));
        }
        if !raw.is_empty() {
            return Ok(TargetKind::Local(raw.to_string()));
        }
    }
    let url = GraphActorUrl::new(raw)?;
    Ok(match local_profile_nanoid(public_base_url, url.as_str()) {
        Some(nanoid) => TargetKind::Local(nanoid.to_string()),
        None => TargetKind::Remote(url),
    })
}

/// 解決済みの target。`local_nanoid` はローカル Profile のときだけ。
struct ResolvedTarget {
    actor: ProfileGraphActor,
    url: String,
    local_nanoid: Option<String>,
}

async fn resolve_target<T: RelationshipDeps>(
    deps: &T,
    executor: &mut Conn<T>,
    kind: &TargetKind,
) -> error_stack::Result<ResolvedTarget, KernelError> {
    match kind {
        TargetKind::Acct(_) => Err(Report::new(KernelError::Internal)
            .attach_printable("an acct target must be resolved before the transaction")),
        TargetKind::Remote(url) => Ok(ResolvedTarget {
            actor: ProfileGraphActor::Remote(url.clone()),
            url: url.as_str().to_string(),
            local_nanoid: None,
        }),
        TargetKind::Local(nanoid) => {
            let profile = deps
                .profile_read_model()
                .find_by_nanoid(executor, &Nanoid::new(nanoid.clone()))
                .await?
                .ok_or_else(|| validation("target is not a known local Profile"))?;
            Ok(ResolvedTarget {
                actor: ProfileGraphActor::Local(profile.id().clone()),
                url: profile_actor_url(deps.public_base_url(), nanoid),
                local_nanoid: Some(nanoid.clone()),
            })
        }
    }
}

/// 関係の当事者の canonical actor URL。ローカル Profile は Profile から導出する。
async fn actor_url<T: RelationshipDeps>(
    deps: &T,
    executor: &mut Conn<T>,
    actor: &ProfileGraphActor,
) -> error_stack::Result<String, KernelError> {
    match actor {
        ProfileGraphActor::Remote(url) => Ok(url.as_str().to_string()),
        ProfileGraphActor::Local(profile_id) => {
            let profile = deps
                .profile_read_model()
                .find_by_id(executor, profile_id)
                .await?
                .ok_or_else(|| not_found("local Profile not found"))?;
            Ok(profile_actor_url(
                deps.public_base_url(),
                profile.nanoid().as_ref(),
            ))
        }
    }
}

fn follow_dto(follow: &ProfileFollow, profile_nanoid: &str, target: String) -> ProfileFollowDto {
    ProfileFollowDto {
        nanoid: follow.nanoid().as_ref().to_string(),
        profile_nanoid: profile_nanoid.to_string(),
        target,
        state: *follow.state(),
        created_at: *follow.created_at(),
    }
}

/// 送信元のローカル Profile (内部 ID と、actor URL の元になる nanoid)。
struct LocalSender<'a> {
    profile_id: &'a ProfileId,
    nanoid: &'a str,
}

/// 自分が送った Follow が終了したとき、相手に `Undo(Follow)` を送る job を予約する (同じ transaction)。
///
/// 宛先 inbox は元の Follow job から取る。まだ一度も送信を試みていない Follow は取り消すだけで
/// Undo を送らない (相手は Follow を知らない)。試行済み (配送済み・再試行中・送信中・失敗) なら、
/// 相手に届いている可能性があるので Undo を送る。送信中の Follow があれば Undo はその後に回す。
async fn retract_outgoing_follow<T: RelationshipDeps>(
    deps: &T,
    executor: &mut Conn<T>,
    sender: &LocalSender<'_>,
    follow: &ProfileFollow,
    target_url: &str,
    reason: &str,
    now: OffsetDateTime,
) -> error_stack::Result<(), KernelError> {
    let jobs = deps.profile_delivery_job_repository();
    let Some(before) = jobs
        .find_by_follow(executor, follow.id(), DeliveryActivityType::Follow)
        .await?
        .into_iter()
        .next()
    else {
        return Ok(());
    };
    jobs.cancel_undelivered_for_follow(
        executor,
        follow.id(),
        DeliveryActivityType::Follow,
        reason,
        now,
    )
    .await?;
    let after = jobs
        .find_by_id(executor, &before.id)
        .await?
        .ok_or_else(|| not_found("delivery job disappeared"))?;
    if after.attempt_count == 0 {
        return Ok(());
    }
    let in_flight_possible = before.state == ProfileDeliveryJobState::InFlight
        || before.attempt_count < after.attempt_count;
    let undo_id = undo_activity_id(follow.activity_id())?;
    let body = undo_follow_body(
        &undo_id,
        &profile_actor_url(deps.public_base_url(), sender.nanoid),
        target_url,
        &before.payload,
    )?;
    let mut undo = NewProfileDeliveryJob::new(
        sender.profile_id.clone(),
        DeliveryActivityType::Undo,
        undo_id,
        before.target_inbox.clone(),
        body,
        now,
    )?
    .with_follow(follow.id().clone());
    if in_flight_possible {
        undo = undo.not_before(now + IN_FLIGHT_GRACE);
    }
    deps.profile_delivery_enqueuer()
        .enqueue(executor, &undo)
        .await?;
    Ok(())
}

/// 相手から受けていた Follow が終了したとき、相手に `Reject(Follow)` を送る job を予約する (同じ
/// transaction)。宛先は、その Follow への Accept job (P3i が予約) の inbox。未送信の Accept は取り消す。
/// Accept job がない (承認前など) 場合は宛先が分からないので何も送らない。
async fn reject_incoming_follow<T: RelationshipDeps>(
    deps: &T,
    executor: &mut Conn<T>,
    sender: &LocalSender<'_>,
    follow: &ProfileFollow,
    reason: &str,
    now: OffsetDateTime,
) -> error_stack::Result<(), KernelError> {
    let jobs = deps.profile_delivery_job_repository();
    // 相手がローカル Profile なら、その Follow がまだ未配送のまま残っていれば取り消す (リモートの相手には
    // この Follow に紐づく自分の job はなく、何も取り消されない)。
    jobs.cancel_undelivered_for_follow(
        executor,
        follow.id(),
        DeliveryActivityType::Follow,
        reason,
        now,
    )
    .await?;
    let Some(accept) = jobs
        .find_by_follow(executor, follow.id(), DeliveryActivityType::Accept)
        .await?
        .into_iter()
        .next()
    else {
        return Ok(());
    };
    jobs.cancel_undelivered_for_follow(
        executor,
        follow.id(),
        DeliveryActivityType::Accept,
        reason,
        now,
    )
    .await?;
    let follower_url = actor_url(deps, executor, follow.follower()).await?;
    let reject_id = reject_activity_id(deps.public_base_url(), sender.nanoid, follow.id())?;
    let body = reject_follow_body(
        &reject_id,
        &profile_actor_url(deps.public_base_url(), sender.nanoid),
        &follower_url,
        follow.activity_id(),
    )?;
    let reject = NewProfileDeliveryJob::new(
        sender.profile_id.clone(),
        DeliveryActivityType::Reject,
        reject_id,
        accept.target_inbox.clone(),
        body,
        now,
    )?
    .with_follow(follow.id().clone());
    deps.profile_delivery_enqueuer()
        .enqueue(executor, &reject)
        .await?;
    Ok(())
}

async fn authorize<T: RelationshipDeps>(
    deps: &T,
    auth_account_id: &AuthAccountId,
    org_context: Option<&OrganizationContext>,
    profile_nanoid: &str,
) -> error_stack::Result<(), KernelError> {
    let owned = deps.clone();
    let auth_account_id = auth_account_id.clone();
    let org_context = org_context.cloned();
    let profile_nanoid = profile_nanoid.to_string();
    deps.transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                require_profile_owner(
                    &owned,
                    executor,
                    &auth_account_id,
                    org_context.as_ref(),
                    &profile_nanoid,
                )
                .await
                .map(|_| ())
            })
        })
        .await
}

/// 関係の判断・変更に関わるローカル Profile を、transaction の先頭で一度だけ昇順にロックする
/// (`kernel::interfaces::repository::ProfileRelationshipLock` の規約)。block 判定と Follow 保存、
/// block 保存と既存 Follow の終了が別 transaction で interleave しないようにする。
async fn lock_relationship<T: RelationshipDeps>(
    deps: &T,
    executor: &mut Conn<T>,
    me: &ProfileId,
    target: &ProfileGraphActor,
) -> error_stack::Result<(), KernelError> {
    let mut profiles = vec![me.clone()];
    if let ProfileGraphActor::Local(target_id) = target {
        profiles.push(target_id.clone());
    }
    deps.profile_relationship_lock()
        .lock_profiles(executor, &profiles)
        .await
}

/// 要求された target の種別を確定する。`acct:` handle は、所有者を認可した後に transaction の外で
/// WebFinger により canonical な actor URL へ解決する (解決できなければ 400)。
async fn prepare_target<T: RelationshipDeps + DependOnRemoteActorResolver>(
    deps: &T,
    auth_account_id: &AuthAccountId,
    org_context: Option<&OrganizationContext>,
    profile_nanoid: &str,
    raw: &str,
) -> error_stack::Result<TargetKind, KernelError> {
    let kind = classify_target(deps.public_base_url(), raw)?;
    let TargetKind::Acct(handle) = kind else {
        return Ok(kind);
    };
    authorize(deps, auth_account_id, org_context, profile_nanoid).await?;
    let actor_url = deps
        .remote_actor_resolver()
        .resolve_acct(&handle)
        .await
        .map_err(|error| match error.current_context() {
            KernelError::Rejected | KernelError::NotFound => error
                .change_context(KernelError::Validation)
                .attach_printable("acct could not be resolved to an actor".to_string()),
            _ => error,
        })?;
    classify_target(deps.public_base_url(), actor_url.as_str())
}

pub trait ProfileRelationshipUseCase: RelationshipDeps + DependOnRemoteActorResolver {
    /// Profile が `target` を Follow する。remote actor 宛ては Follow 世代 (Accept 待ち) の保存と `Follow`
    /// activity の配送 job 予約を同一 transaction で行う。ローカル Profile 宛ては HTTP を経由せず、同じ
    /// transaction で承認済みの世代を確定する (配送 job なし)。target が自分・block 関係にある (どちらの向きでも) 場合は拒否 (422)、
    /// 有効な Follow が既にあれば 409。remote actor の inbox 解決 (HTTP) は transaction の外で行い、
    /// 解決できない target は 400。
    fn follow_profile<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: &'a str,
        target: &'a str,
    ) -> impl Future<Output = error_stack::Result<ProfileFollowDto, KernelError>> + Send + 'a {
        async move {
            let kind =
                prepare_target(self, auth_account_id, org_context, profile_nanoid, target).await?;
            authorize(self, auth_account_id, org_context, profile_nanoid).await?;
            let remote_inbox = match &kind {
                TargetKind::Remote(url) => Some(
                    self.remote_actor_resolver()
                        .resolve(url)
                        .await
                        .map_err(|error| match error.current_context() {
                            KernelError::Rejected | KernelError::NotFound => error
                                .change_context(KernelError::Validation)
                                .attach_printable(
                                    "follow target could not be resolved to an actor".to_string(),
                                ),
                            _ => error,
                        })?,
                ),
                TargetKind::Local(_) | TargetKind::Acct(_) => None,
            };
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
                        let now = now();
                        let me = ProfileGraphActor::Local(profile_id.clone());
                        let target = resolve_target(&deps, executor, &kind).await?;
                        if target.actor == me {
                            return Err(validation("a Profile cannot follow itself"));
                        }
                        lock_relationship(&deps, executor, &profile_id, &target.actor).await?;
                        let blocks = deps.profile_block_repository();
                        let blocked = blocks
                            .is_blocked(executor, &profile_id, &target.actor)
                            .await?
                            || match &target.actor {
                                ProfileGraphActor::Local(target_id) => {
                                    blocks.is_blocked(executor, target_id, &me).await?
                                }
                                ProfileGraphActor::Remote(_) => false,
                            };
                        if blocked {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("follow refused by an individual block"));
                        }
                        // Domain `block` (D8G), decided under the same lock: the follower's own
                        // block of the target's host, and for a local target also the target's
                        // block of the follower's host. Both sides use the verified canonical actor
                        // URL (a local actor URL carries this instance's host). `mute` is ignored.
                        let my_actor_url =
                            profile_actor_url(deps.public_base_url(), &profile_nanoid);
                        let domain_blocked =
                            domain_blocks_actor(&deps, executor, &profile_id, &target.url).await?
                                || match &target.actor {
                                    ProfileGraphActor::Local(target_id) => {
                                        domain_blocks_actor(
                                            &deps,
                                            executor,
                                            target_id,
                                            &my_actor_url,
                                        )
                                        .await?
                                    }
                                    ProfileGraphActor::Remote(_) => false,
                                };
                        if domain_blocked {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("follow refused by a domain block"));
                        }
                        let follow_id = ProfileFollowId::new(kernel::generate_id());
                        let activity_id = follow_activity_id(
                            deps.public_base_url(),
                            &profile_nanoid,
                            &follow_id,
                        )?;
                        // ローカル Profile 宛ては HTTP を経由せず、同じ transaction (両 Profile の関係ロック下)
                        // で確定する。受信側 (P3i) の Follow 処理と同じ意味論で、承認不要の Profile として
                        // 承認済みの世代を `Local(ProfileId)` 同士で保存する (相手の個別 block は上で検査済み)。
                        // 自インスタンスの actor からの HTTP 受信は P3i が 422 で拒否するので、配送 job は作らない。
                        let is_local_target = target.local_nanoid.is_some();
                        let follow = if is_local_target {
                            ProfileFollow::new_accepted(
                                follow_id.clone(),
                                me,
                                target.actor.clone(),
                                activity_id.clone(),
                                now,
                            )?
                        } else {
                            ProfileFollow::new_pending(
                                follow_id.clone(),
                                me,
                                target.actor.clone(),
                                activity_id.clone(),
                                now,
                            )?
                        };
                        match deps
                            .profile_follow_repository()
                            .begin_generation(executor, &follow)
                            .await?
                        {
                            BeginFollowOutcome::Created => {}
                            BeginFollowOutcome::ReplayedActivity(_)
                            | BeginFollowOutcome::ActiveGenerationExists(_) => {
                                return Err(conflict("an active follow already exists"))
                            }
                        }
                        if is_local_target {
                            return Ok(follow_dto(&follow, &profile_nanoid, target.url));
                        }
                        let Some(remote) = remote_inbox else {
                            return Err(Report::new(KernelError::Internal)
                                .attach_printable("remote target without a resolved inbox"));
                        };
                        let body = follow_body(
                            &activity_id,
                            &profile_actor_url(deps.public_base_url(), &profile_nanoid),
                            &target.url,
                        )?;
                        let job = NewProfileDeliveryJob::new(
                            profile_id,
                            DeliveryActivityType::Follow,
                            activity_id,
                            remote.inbox,
                            body,
                            now,
                        )?
                        .with_follow(follow_id);
                        deps.profile_delivery_enqueuer()
                            .enqueue(executor, &job)
                            .await?;
                        Ok(follow_dto(&follow, &profile_nanoid, target.url))
                    })
                })
                .await
        }
    }

    /// Profile が行っている Follow (`follow_nanoid`) を解除する。Follow 世代を `Undone` にし、元 Follow
    /// activity を参照する `Undo(Follow)` の配送 job を同一 transaction で予約する。配送の成否にかかわらず
    /// ローカルの解除は戻らない。自分の有効な Follow でなければ 404。
    fn unfollow_profile<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: &'a str,
        follow_nanoid: &'a str,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            let deps = self.clone();
            let auth_account_id = auth_account_id.clone();
            let org_context = org_context.cloned();
            let profile_nanoid = profile_nanoid.to_string();
            let follow_nanoid = follow_nanoid.to_string();
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
                        // 関係ロックは所有者確認の直後・Follow の読取り前。配送前の domain 判定は
                        // 「job -> Follow 世代」の順に更新するので、ここで「Follow 世代 -> job」をロックなしに
                        // 更新すると行ロックが循環してデッドロックする。
                        deps.profile_relationship_lock()
                            .lock_profiles(executor, std::slice::from_ref(&profile_id))
                            .await?;
                        let now = now();
                        let me = ProfileGraphActor::Local(profile_id.clone());
                        let follows = deps.profile_follow_repository();
                        let follow = follows
                            .find_by_nanoid(executor, &Nanoid::new(follow_nanoid.clone()))
                            .await?
                            .filter(|follow| follow.follower() == &me && follow.is_active())
                            .ok_or_else(|| not_found("follow not found"))?;
                        if !follows
                            .end(executor, follow.id(), ProfileFollowEndReason::Undone, now)
                            .await?
                        {
                            return Err(not_found("follow not found"));
                        }
                        let target_url = actor_url(&deps, executor, follow.followee()).await?;
                        retract_outgoing_follow(
                            &deps,
                            executor,
                            &LocalSender {
                                profile_id: &profile_id,
                                nanoid: &profile_nanoid,
                            },
                            &follow,
                            &target_url,
                            "unfollowed",
                            now,
                        )
                        .await
                    })
                })
                .await
        }
    }

    /// Profile が個別の actor を block する。block 設定の保存、双方向の有効な Follow の解除、解除した Follow を
    /// 相手へ伝える job (`Undo(Follow)` / `Reject(Follow)`) の予約を同一 transaction で行う。AP `Block` は送らない。
    /// 既に block していれば 409。
    fn block_actor<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: &'a str,
        target: &'a str,
    ) -> impl Future<Output = error_stack::Result<ProfileRelationDto, KernelError>> + Send + 'a
    {
        async move {
            let kind =
                prepare_target(self, auth_account_id, org_context, profile_nanoid, target).await?;
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
                        let now = now();
                        let me = ProfileGraphActor::Local(profile_id.clone());
                        let target = resolve_target(&deps, executor, &kind).await?;
                        lock_relationship(&deps, executor, &profile_id, &target.actor).await?;
                        let block = ProfileBlock::new(
                            ProfileBlockId::new(kernel::generate_id()),
                            profile_id.clone(),
                            target.actor.clone(),
                            now,
                        )
                        .map_err(|error| error.change_context(KernelError::Validation))?;
                        if !deps
                            .profile_block_repository()
                            .insert_if_absent(executor, &block)
                            .await?
                        {
                            return Err(conflict("target is already blocked"));
                        }
                        let ended = deps
                            .profile_follow_repository()
                            .end_active_between(
                                executor,
                                &me,
                                &target.actor,
                                ProfileFollowEndReason::Blocked,
                                now,
                            )
                            .await?;
                        let sender = LocalSender {
                            profile_id: &profile_id,
                            nanoid: &profile_nanoid,
                        };
                        for follow in &ended {
                            if follow.follower() == &me {
                                retract_outgoing_follow(
                                    &deps,
                                    executor,
                                    &sender,
                                    follow,
                                    &target.url,
                                    "blocked",
                                    now,
                                )
                                .await?;
                            } else {
                                reject_incoming_follow(
                                    &deps, executor, &sender, follow, "blocked", now,
                                )
                                .await?;
                            }
                        }
                        Ok(ProfileRelationDto {
                            nanoid: block.nanoid().as_ref().to_string(),
                            profile_nanoid,
                            target: target.url,
                            created_at: *block.created_at(),
                        })
                    })
                })
                .await
        }
    }

    /// block を解除する (自分の設定だけ。他 Profile の設定は消えない。無ければ 404)。解除済みの Follow は復元せず、
    /// 何も配送しない。
    fn unblock_actor<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: &'a str,
        block_nanoid: &'a str,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            let deps = self.clone();
            let auth_account_id = auth_account_id.clone();
            let org_context = org_context.cloned();
            let profile_nanoid = profile_nanoid.to_string();
            let block_nanoid = block_nanoid.to_string();
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
                        if deps
                            .profile_block_repository()
                            .delete_by_nanoid(executor, &profile_id, &Nanoid::new(block_nanoid))
                            .await?
                        {
                            Ok(())
                        } else {
                            Err(not_found("block not found"))
                        }
                    })
                })
                .await
        }
    }

    /// Profile が個別の actor を mute する (表示抑制の設定だけ。Follow も配送も変えない)。既に mute していれば 409。
    fn mute_actor<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: &'a str,
        target: &'a str,
    ) -> impl Future<Output = error_stack::Result<ProfileRelationDto, KernelError>> + Send + 'a
    {
        async move {
            let kind =
                prepare_target(self, auth_account_id, org_context, profile_nanoid, target).await?;
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
                        let target = resolve_target(&deps, executor, &kind).await?;
                        let mute = ProfileMute::new(
                            ProfileMuteId::new(kernel::generate_id()),
                            profile_id,
                            target.actor,
                            now(),
                        )
                        .map_err(|error| error.change_context(KernelError::Validation))?;
                        if !deps
                            .profile_mute_repository()
                            .insert_if_absent(executor, &mute)
                            .await?
                        {
                            return Err(conflict("target is already muted"));
                        }
                        Ok(ProfileRelationDto {
                            nanoid: mute.nanoid().as_ref().to_string(),
                            profile_nanoid,
                            target: target.url,
                            created_at: *mute.created_at(),
                        })
                    })
                })
                .await
        }
    }

    fn unmute_actor<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: &'a str,
        mute_nanoid: &'a str,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            let deps = self.clone();
            let auth_account_id = auth_account_id.clone();
            let org_context = org_context.cloned();
            let profile_nanoid = profile_nanoid.to_string();
            let mute_nanoid = mute_nanoid.to_string();
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
                        if deps
                            .profile_mute_repository()
                            .delete_by_nanoid(executor, &profile_id, &Nanoid::new(mute_nanoid))
                            .await?
                        {
                            Ok(())
                        } else {
                            Err(not_found("mute not found"))
                        }
                    })
                })
                .await
        }
    }
}

impl<T> ProfileRelationshipUseCase for T where T: RelationshipDeps + DependOnRemoteActorResolver {}
