//! domain `block` の設定時に、対象 domain の actor との既存の関係を両方向に解除する (D8R)。
//!
//! `put_domain_control` が mode=block を保存する **同じ transaction** で次を行う (どれか一つでも失敗
//! すれば全体が rollback され、block だけ・解除だけが残らない):
//!
//! ロック順: Profile 行 `FOR SHARE` → advisory (`lock_profiles`) → block 保存 → Follow 行 (`end_active_between`) → job 行
//! (取消・予約)。配送前ガード (`delivery`) は job → Follow の順で更新するが、同じ Profile の advisory ロックを
//! 先に取るので、この解除とは直列化され、行ロックの循環は起きない (`unfollow` と同じ構造)。`renew_lease` /
//! `record_attempt` は job 行だけを触るので Follow 行を待たない。
//!
//! 1. 所有 Profile (と、自 host の domain を block する場合は host が一致するローカル相手 Profile) の
//!    関係ロックを昇順の単一 `lock_profiles` で取る ([`lock_for_block_write`])。
//! 2. ロック下で、Profile の有効な Follow (pending 含む・following / followers 両方向) のうち相手の検証済み
//!    canonical actor ID の host が block 対象と完全一致するものを読む ([`matching_counterparts`])。
//! 3. block を保存した後、相手ごとに `end_active_between` で両方向の有効な世代を `blocked` で終了し、関係の
//!    作者に応じた後始末を予約する ([`detach_counterparts`]):
//!    * 自分が送った Follow → 元 Follow を参照する `Undo(Follow)` (未送信の Follow は取り消すだけ。
//!      送信済み・結果不明は `Undo`、送信中だった可能性があれば遅延)
//!    * 相手が送った Follow → 元 Follow を参照する `Reject(Follow)` (未送信の Accept は取り消し、送信中
//!      だった可能性があれば遅延)。相手作成の Follow を自分が作者であるかのように Undo しない
//!    * ローカル相手との関係は終了だけで、配送はない
//!
//! AP `Block` は作らない。block を解除しても関係は復元せず、復元用の一覧も持たない。外部配送は
//! 関係状態とは独立で、worker の成否は block・解除のローカル適用に影響しない。

use super::activity::profile_actor_url;
use super::relationship::{
    actor_url, now, reject_incoming_follow, retract_outgoing_follow, Conn, LocalSender,
    RelationshipDeps,
};
use kernel::interfaces::repository::{ProfileFollowRepository, ProfileRelationshipLock};
use kernel::prelude::entity::{
    ControlDomain, ProfileFollow, ProfileFollowEndReason, ProfileGraphActor, ProfileId,
};
use kernel::KernelError;
use time::OffsetDateTime;

const PAGE: i64 = 500;

#[cfg(test)]
thread_local! {
    /// Number of abandoned attempts (`LockOutcome::Retry`) on this thread; lets a test observe the retry branch.
    pub(crate) static RETRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 解除する関係の相手 (block 対象 domain の actor)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Counterpart {
    pub actor: ProfileGraphActor,
    pub actor_url: String,
}

/// この instance の actor URL (`{PUBLIC_BASE_URL}/ap/profiles/…`) の host が `domain` か。
fn blocks_own_host<T: RelationshipDeps>(deps: &T, domain: &ControlDomain) -> bool {
    let sample = profile_actor_url(deps.public_base_url(), "x");
    ControlDomain::from_actor_id(&sample).is_ok_and(|own| &own == domain)
}

/// `profile_id` の有効な Follow の相手 (following・followers の両方向) のうち、検証済み canonical
/// actor ID の host が `domain` と完全一致するもの。相手ごとに一度だけ返し、ID 順に安定する。
///
/// 関係ロック下で呼ぶこと (呼び出し側が与えたロックが、読んだ関係の増減を直列化する)。
pub(crate) async fn matching_counterparts<T: RelationshipDeps>(
    deps: &T,
    executor: &mut Conn<T>,
    profile_id: &ProfileId,
    domain: &ControlDomain,
) -> error_stack::Result<Vec<Counterpart>, KernelError> {
    let own_host_blocked = blocks_own_host(deps, domain);
    let me = ProfileGraphActor::Local(profile_id.clone());
    let mut follows: Vec<ProfileFollow> = Vec::new();
    for following in [true, false] {
        let mut after = None;
        loop {
            let repository = deps.profile_follow_repository();
            let page = if following {
                repository
                    .list_following(executor, profile_id, after.as_ref(), PAGE)
                    .await?
            } else {
                repository
                    .list_followers(executor, profile_id, after.as_ref(), PAGE)
                    .await?
            };
            let Some(last) = page.last() else { break };
            after = Some(last.id().clone());
            let count = page.len() as i64;
            follows.extend(page);
            if count < PAGE {
                break;
            }
        }
    }
    let mut counterparts: Vec<Counterpart> = Vec::new();
    for follow in &follows {
        let other = if follow.follower() == &me {
            follow.followee()
        } else {
            follow.follower()
        };
        if counterparts.iter().any(|known| &known.actor == other) {
            continue;
        }
        let url = match other {
            ProfileGraphActor::Remote(url) => url.as_str().to_string(),
            ProfileGraphActor::Local(_) if !own_host_blocked => continue,
            ProfileGraphActor::Local(_) => match actor_url(deps, executor, other).await {
                Ok(url) => url,
                // A Profile that no longer exists has no actor and no relation to detach.
                Err(error) if matches!(error.current_context(), KernelError::NotFound) => continue,
                Err(error) => return Err(error),
            },
        };
        if ControlDomain::from_actor_id(&url).is_ok_and(|host| &host == domain) {
            counterparts.push(Counterpart {
                actor: other.clone(),
                actor_url: url,
            });
        }
    }
    Ok(counterparts)
}

/// 関係ロックの取得結果。
pub(crate) enum LockOutcome {
    /// 取得できた。`counterparts` はロック下で読んだ解除対象 (block 設定時のみ)。
    Locked { counterparts: Vec<Counterpart> },
    /// ロックを取る前に見えなかったローカル相手が現れた (別 transaction の Follow)。何も書かずに
    /// transaction をやり直す。
    Retry,
}

/// domain control の書込 transaction の関係ロックを取る。`detaching` が false (mute への置換・削除) は
/// 所有 Profile だけをロックする (D8G の規約どおり)。`detaching` が true (block の保存) は、自 host の
/// domain を block する場合に限り host が一致するローカル相手 Profile も同じ昇順の単一呼び出しでロックし、
/// ロック下で解除対象を確定する。
pub(crate) async fn lock_for_block_write<T: RelationshipDeps>(
    deps: &T,
    executor: &mut Conn<T>,
    profile_id: &ProfileId,
    domain: &ControlDomain,
    detaching: bool,
) -> error_stack::Result<LockOutcome, KernelError> {
    if !detaching {
        crate::service::domain_follow_guard::lock_profile_for_domain_control_write(
            deps, executor, profile_id,
        )
        .await?;
        return Ok(LockOutcome::Locked {
            counterparts: Vec::new(),
        });
    }
    // The first read decides which local Profiles to lock, so it cannot be under the lock yet. It
    // is re-read under the lock below; a local counterpart that appeared in between triggers a retry.
    let mut locked: Vec<ProfileId> = vec![profile_id.clone()];
    if blocks_own_host(deps, domain) {
        for counterpart in matching_counterparts(deps, executor, profile_id, domain).await? {
            if let ProfileGraphActor::Local(id) = counterpart.actor {
                locked.push(id);
            }
        }
    }
    deps.profile_relationship_lock()
        .lock_profiles(executor, &locked)
        .await?;
    let counterparts = matching_counterparts(deps, executor, profile_id, domain).await?;
    let unlocked_local = counterparts.iter().any(|counterpart| {
        matches!(&counterpart.actor, ProfileGraphActor::Local(id) if !locked.contains(id))
    });
    if unlocked_local {
        #[cfg(test)]
        RETRIES.with(|retries| retries.set(retries.get() + 1));
        return Ok(LockOutcome::Retry);
    }
    Ok(LockOutcome::Locked { counterparts })
}

/// block の保存と同じ transaction で、`counterparts` との有効な関係を両方向とも終了し (`blocked`)、
/// 関係の作者に応じた後始末 job を予約する。
pub(crate) async fn detach_counterparts<T: RelationshipDeps>(
    deps: &T,
    executor: &mut Conn<T>,
    profile_id: &ProfileId,
    profile_nanoid: &str,
    counterparts: &[Counterpart],
) -> error_stack::Result<(), KernelError> {
    if counterparts.is_empty() {
        return Ok(());
    }
    let now: OffsetDateTime = now();
    let me = ProfileGraphActor::Local(profile_id.clone());
    let sender = LocalSender {
        profile_id,
        nanoid: profile_nanoid,
    };
    for counterpart in counterparts {
        let ended = deps
            .profile_follow_repository()
            .end_active_between(
                executor,
                &me,
                &counterpart.actor,
                ProfileFollowEndReason::Blocked,
                now,
            )
            .await?;
        for follow in &ended {
            if follow.follower() == &me {
                retract_outgoing_follow(
                    deps,
                    executor,
                    &sender,
                    follow,
                    &counterpart.actor_url,
                    "domain blocked",
                    now,
                )
                .await?;
            } else {
                reject_incoming_follow(deps, executor, &sender, follow, "domain blocked", now)
                    .await?;
            }
        }
    }
    Ok(())
}
