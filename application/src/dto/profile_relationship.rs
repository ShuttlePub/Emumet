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

/// block / mute の対象 actor の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationActorKind {
    Local,
    Remote,
}

/// 対象 actor の表示情報の取得状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationActorStatus {
    /// 表示情報がある。
    Resolved,
    /// remote で未取得 (取得予定または取得中)。
    Pending,
    /// remote で直近の取得が失敗 (期限後に再試行される)。
    Unavailable,
}

/// block / mute の対象 actor の表示情報 (local は Profile から、remote はキャッシュから)。
/// `Resolved` 以外では表示情報の各項目は `None`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationActorDto {
    pub kind: RelationActorKind,
    pub actor_url: String,
    /// local のときだけ Profile の nanoid。
    pub profile_nanoid: Option<String>,
    pub status: RelationActorStatus,
    /// `preferred_username@host` (先頭の `@` なし)。
    pub acct: Option<String>,
    pub preferred_username: Option<String>,
    pub display_name: Option<String>,
    pub icon_url: Option<String>,
}

/// 個別 block / mute の設定と、その対象 actor の表示情報。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRelationItemDto {
    pub relation: ProfileRelationDto,
    pub target_actor: RelationActorDto,
}
