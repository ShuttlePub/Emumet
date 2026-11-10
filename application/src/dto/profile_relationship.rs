use kernel::prelude::entity::ProfileFollowState;
use time::OffsetDateTime;

/// Profile が行っている Follow (following 一覧・作成結果)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileFollowDto {
    /// Follow 世代の API 上の ID (nanoid)。
    pub nanoid: String,
    pub profile_nanoid: String,
    /// フォロー先の canonical actor URL。
    pub target: String,
    pub state: ProfileFollowState,
    pub created_at: OffsetDateTime,
}

/// 個別 actor の block / mute 設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRelationDto {
    pub nanoid: String,
    pub profile_nanoid: String,
    pub target: String,
    pub created_at: OffsetDateTime,
}
