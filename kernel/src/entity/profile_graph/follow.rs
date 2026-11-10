use crate::entity::{GraphActivityId, Nanoid, ProfileGraphActor};
use crate::KernelError;
use error_stack::Report;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use vodca::{AsRefln, Fromln, Newln, References};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct ProfileFollowId(i64);

/// Follow 1 世代の状態。`Ended` は終端で、同じ世代は復活しない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileFollowState {
    /// Follow を送信/受信済みで Accept 待ち。
    Pending,
    Accepted,
    Ended,
}

impl ProfileFollowState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Ended => "ended",
        }
    }

    pub fn parse(value: &str) -> error_stack::Result<Self, KernelError> {
        match value {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "ended" => Ok(Self::Ended),
            other => Err(Report::new(KernelError::Internal)
                .attach_printable(format!("unknown profile follow state: {other}"))),
        }
    }
}

/// 世代が終了した理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileFollowEndReason {
    /// フォロワー側が取り消した (Undo / unfollow)。
    Undone,
    /// フォロイー側が拒否・解除した (Reject)。
    Rejected,
    /// block により双方向の関係が解除された。block を解除しても復元しない。
    Blocked,
}

impl ProfileFollowEndReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Undone => "undone",
            Self::Rejected => "rejected",
            Self::Blocked => "blocked",
        }
    }

    pub fn parse(value: &str) -> error_stack::Result<Self, KernelError> {
        match value {
            "undone" => Ok(Self::Undone),
            "rejected" => Ok(Self::Rejected),
            "blocked" => Ok(Self::Blocked),
            other => Err(Report::new(KernelError::Internal)
                .attach_printable(format!("unknown profile follow end reason: {other}"))),
        }
    }
}

/// Profile 版 Follow の 1 世代。
///
/// 同じ (follower, followee) の組に対して、**元 Follow activity ID ごとに別の世代**を持つ。
/// 有効な世代 (`Ended` 以外) は組ごとに高々 1 つ。終了した世代は activity ID とともに残すので、
/// block 解除後などに古い activity が再送されても世代は復活しない。
#[derive(Debug, Clone, PartialEq, Eq, Hash, References, Serialize, Deserialize)]
pub struct ProfileFollow {
    id: ProfileFollowId,
    nanoid: Nanoid<ProfileFollow>,
    follower: ProfileGraphActor,
    followee: ProfileGraphActor,
    activity_id: GraphActivityId,
    state: ProfileFollowState,
    end_reason: Option<ProfileFollowEndReason>,
    #[serde(with = "time::serde::rfc3339::option")]
    accepted_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    ended_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

/// DB 行などから `ProfileFollow` を復元するための全フィールド。
#[derive(Debug, Clone)]
pub struct ProfileFollowParts {
    pub id: ProfileFollowId,
    pub nanoid: Nanoid<ProfileFollow>,
    pub follower: ProfileGraphActor,
    pub followee: ProfileGraphActor,
    pub activity_id: GraphActivityId,
    pub state: ProfileFollowState,
    pub end_reason: Option<ProfileFollowEndReason>,
    pub accepted_at: Option<OffsetDateTime>,
    pub ended_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl ProfileFollow {
    /// Accept 待ちの新しい世代。
    pub fn new_pending(
        id: ProfileFollowId,
        follower: ProfileGraphActor,
        followee: ProfileGraphActor,
        activity_id: GraphActivityId,
        now: OffsetDateTime,
    ) -> error_stack::Result<Self, KernelError> {
        Self::from_parts(ProfileFollowParts {
            id,
            nanoid: Nanoid::default(),
            follower,
            followee,
            activity_id,
            state: ProfileFollowState::Pending,
            end_reason: None,
            accepted_at: None,
            ended_at: None,
            created_at: now,
            updated_at: now,
        })
    }

    /// 即時承認する新しい世代 (承認不要の Profile が受信した Follow など)。
    pub fn new_accepted(
        id: ProfileFollowId,
        follower: ProfileGraphActor,
        followee: ProfileGraphActor,
        activity_id: GraphActivityId,
        now: OffsetDateTime,
    ) -> error_stack::Result<Self, KernelError> {
        Self::from_parts(ProfileFollowParts {
            id,
            nanoid: Nanoid::default(),
            follower,
            followee,
            activity_id,
            state: ProfileFollowState::Accepted,
            end_reason: None,
            accepted_at: Some(now),
            ended_at: None,
            created_at: now,
            updated_at: now,
        })
    }

    /// 不変条件 (remote 同士の関係なし・自己 Follow なし・状態と時刻の整合) を検査して復元する。
    pub fn from_parts(parts: ProfileFollowParts) -> error_stack::Result<Self, KernelError> {
        let invalid = |message: &str| {
            Report::new(KernelError::Validation)
                .attach_printable(format!("invalid profile follow: {message}"))
        };
        match (&parts.follower, &parts.followee) {
            (ProfileGraphActor::Remote(_), ProfileGraphActor::Remote(_)) => {
                return Err(invalid("remote to remote follow is not stored"))
            }
            (ProfileGraphActor::Local(a), ProfileGraphActor::Local(b)) if a == b => {
                return Err(invalid("a profile cannot follow itself"))
            }
            _ => {}
        }
        let consistent = match parts.state {
            ProfileFollowState::Pending => {
                parts.end_reason.is_none()
                    && parts.ended_at.is_none()
                    && parts.accepted_at.is_none()
            }
            ProfileFollowState::Accepted => {
                parts.end_reason.is_none()
                    && parts.ended_at.is_none()
                    && parts.accepted_at.is_some()
            }
            ProfileFollowState::Ended => parts.end_reason.is_some() && parts.ended_at.is_some(),
        };
        if !consistent {
            return Err(invalid("state does not match end reason / timestamps"));
        }
        Ok(Self {
            id: parts.id,
            nanoid: parts.nanoid,
            follower: parts.follower,
            followee: parts.followee,
            activity_id: parts.activity_id,
            state: parts.state,
            end_reason: parts.end_reason,
            accepted_at: parts.accepted_at,
            ended_at: parts.ended_at,
            created_at: parts.created_at,
            updated_at: parts.updated_at,
        })
    }

    /// Accept 待ちまたは承認済み (終了していない)。
    pub fn is_active(&self) -> bool {
        self.state != ProfileFollowState::Ended
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{GraphActorUrl, ProfileId};

    fn now() -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn local(id: i64) -> ProfileGraphActor {
        ProfileGraphActor::Local(ProfileId::new(id))
    }

    fn remote(path: &str) -> ProfileGraphActor {
        ProfileGraphActor::Remote(
            GraphActorUrl::new(format!("https://remote.example/{path}")).unwrap(),
        )
    }

    fn activity(n: u32) -> GraphActivityId {
        GraphActivityId::new(format!("https://remote.example/activities/{n}")).unwrap()
    }

    #[test]
    fn rejects_remote_to_remote_and_self_follow() {
        assert!(ProfileFollow::new_pending(
            ProfileFollowId::new(1),
            remote("a"),
            remote("b"),
            activity(1),
            now()
        )
        .is_err());
        assert!(ProfileFollow::new_pending(
            ProfileFollowId::new(1),
            local(1),
            local(1),
            activity(1),
            now()
        )
        .is_err());
    }

    #[test]
    fn local_remote_in_both_directions_is_valid() {
        let outbound = ProfileFollow::new_pending(
            ProfileFollowId::new(1),
            local(1),
            remote("bob"),
            activity(1),
            now(),
        )
        .unwrap();
        assert_eq!(outbound.state(), &ProfileFollowState::Pending);
        assert!(outbound.is_active());
        let inbound = ProfileFollow::new_accepted(
            ProfileFollowId::new(2),
            remote("bob"),
            local(1),
            activity(2),
            now(),
        )
        .unwrap();
        assert!(inbound.accepted_at().is_some());
    }

    #[test]
    fn from_parts_rejects_inconsistent_state() {
        let base = ProfileFollowParts {
            id: ProfileFollowId::new(1),
            nanoid: Nanoid::default(),
            follower: local(1),
            followee: remote("bob"),
            activity_id: activity(1),
            state: ProfileFollowState::Ended,
            end_reason: None,
            accepted_at: None,
            ended_at: None,
            created_at: now(),
            updated_at: now(),
        };
        assert!(ProfileFollow::from_parts(base.clone()).is_err());
        let ended = ProfileFollowParts {
            end_reason: Some(ProfileFollowEndReason::Undone),
            ended_at: Some(now()),
            ..base
        };
        let follow = ProfileFollow::from_parts(ended).unwrap();
        assert!(!follow.is_active());
    }

    #[test]
    fn enum_text_round_trip() {
        for state in [
            ProfileFollowState::Pending,
            ProfileFollowState::Accepted,
            ProfileFollowState::Ended,
        ] {
            assert_eq!(ProfileFollowState::parse(state.as_str()).unwrap(), state);
        }
        for reason in [
            ProfileFollowEndReason::Undone,
            ProfileFollowEndReason::Rejected,
            ProfileFollowEndReason::Blocked,
        ] {
            assert_eq!(
                ProfileFollowEndReason::parse(reason.as_str()).unwrap(),
                reason
            );
        }
        assert!(ProfileFollowState::parse("nope").is_err());
    }
}
