use crate::entity::{Nanoid, ProfileGraphActor, ProfileId};
use crate::KernelError;
use error_stack::Report;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use vodca::{AsRefln, Fromln, Newln, References};

fn ensure_not_self(
    kind: &str,
    owner: &ProfileId,
    target: &ProfileGraphActor,
) -> error_stack::Result<(), KernelError> {
    if target.local_profile_id() == Some(owner) {
        return Err(Report::new(KernelError::Validation)
            .attach_printable(format!("a profile cannot {kind} itself")));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct ProfileBlockId(i64);

/// Profile が個別の actor を block する設定。主体は常に `Local(ProfileId)`。
///
/// block は表示抑制、双方向の Follow 解除 (`ProfileFollowEndReason::Blocked`)、新規操作の拒否を
/// 意味するが、それらの適用は use case の責務で、この型は設定の保存だけを表す。
/// block を解除しても解除済みの関係は復元しない。AP `Block` は個別 actor の block だけが送る
/// (domain block は送らない)。
#[derive(Debug, Clone, PartialEq, Eq, Hash, References, Serialize, Deserialize)]
pub struct ProfileBlock {
    id: ProfileBlockId,
    nanoid: Nanoid<ProfileBlock>,
    owner: ProfileId,
    target: ProfileGraphActor,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

impl ProfileBlock {
    pub fn new(
        id: ProfileBlockId,
        owner: ProfileId,
        target: ProfileGraphActor,
        created_at: OffsetDateTime,
    ) -> error_stack::Result<Self, KernelError> {
        Self::restore(id, Nanoid::default(), owner, target, created_at)
    }

    pub fn restore(
        id: ProfileBlockId,
        nanoid: Nanoid<ProfileBlock>,
        owner: ProfileId,
        target: ProfileGraphActor,
        created_at: OffsetDateTime,
    ) -> error_stack::Result<Self, KernelError> {
        ensure_not_self("block", &owner, &target)?;
        Ok(Self {
            id,
            nanoid,
            owner,
            target,
            created_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct ProfileMuteId(i64);

/// Profile が個別の actor を mute する設定 (表示抑制のみ。既存の関係は変えない)。
#[derive(Debug, Clone, PartialEq, Eq, Hash, References, Serialize, Deserialize)]
pub struct ProfileMute {
    id: ProfileMuteId,
    nanoid: Nanoid<ProfileMute>,
    owner: ProfileId,
    target: ProfileGraphActor,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

impl ProfileMute {
    pub fn new(
        id: ProfileMuteId,
        owner: ProfileId,
        target: ProfileGraphActor,
        created_at: OffsetDateTime,
    ) -> error_stack::Result<Self, KernelError> {
        Self::restore(id, Nanoid::default(), owner, target, created_at)
    }

    pub fn restore(
        id: ProfileMuteId,
        nanoid: Nanoid<ProfileMute>,
        owner: ProfileId,
        target: ProfileGraphActor,
        created_at: OffsetDateTime,
    ) -> error_stack::Result<Self, KernelError> {
        ensure_not_self("mute", &owner, &target)?;
        Ok(Self {
            id,
            nanoid,
            owner,
            target,
            created_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cannot_block_or_mute_self() {
        let owner = ProfileId::new(1);
        let now = OffsetDateTime::now_utc();
        assert!(ProfileBlock::new(
            ProfileBlockId::new(1),
            owner.clone(),
            ProfileGraphActor::Local(owner.clone()),
            now
        )
        .is_err());
        assert!(ProfileMute::new(
            ProfileMuteId::new(1),
            owner.clone(),
            ProfileGraphActor::Local(owner.clone()),
            now
        )
        .is_err());
        assert!(ProfileBlock::new(
            ProfileBlockId::new(1),
            owner,
            ProfileGraphActor::Local(ProfileId::new(2)),
            now
        )
        .is_ok());
    }
}
