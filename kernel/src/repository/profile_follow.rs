use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    GraphActivityId, Nanoid, ProfileFollow, ProfileFollowEndReason, ProfileFollowId,
    ProfileGraphActor, ProfileId,
};
use crate::KernelError;
use std::future::Future;
use time::OffsetDateTime;

/// `ProfileFollowRepository::begin_generation` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeginFollowOutcome {
    /// 新しい世代を保存した。
    Created,
    /// 同じ (follower, activity ID) の世代が既にある (再送・再受信)。状態は問わず、何も変更しない。
    /// 終了済みの世代もここに入るので、古い activity の再送で世代は復活しない。
    ReplayedActivity(Box<ProfileFollow>),
    /// 同じ (follower, followee) の有効な世代が別の activity ID で既にある。何も変更しない。
    ActiveGenerationExists(Box<ProfileFollow>),
}

/// Profile 版 Follow の保存 port。
///
/// すべての操作は `executor` の接続 (transaction を含む) 上で動く。関係の変更と配送 job の予約
/// (`ProfileDeliveryEnqueuer::enqueue`) に同じ transaction 接続を渡せば、両者は同時に commit /
/// rollback される。
pub trait ProfileFollowRepository: Sync + Send + 'static {
    type Connection: Connection;

    /// 新しい世代を保存する。同じ組の有効な世代は高々 1 つで、同時実行しても片方だけが `Created` になる。
    fn begin_generation(
        &self,
        executor: &mut Self::Connection,
        follow: &ProfileFollow,
    ) -> impl Future<Output = error_stack::Result<BeginFollowOutcome, KernelError>> + Send;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileFollowId,
    ) -> impl Future<Output = error_stack::Result<Option<ProfileFollow>, KernelError>> + Send;

    /// API 上の ID (nanoid) による取得。
    fn find_by_nanoid(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<ProfileFollow>,
    ) -> impl Future<Output = error_stack::Result<Option<ProfileFollow>, KernelError>> + Send;

    /// follower が発行した元 Follow activity ID から世代を引く。
    fn find_by_follower_activity(
        &self,
        executor: &mut Self::Connection,
        follower: &ProfileGraphActor,
        activity_id: &GraphActivityId,
    ) -> impl Future<Output = error_stack::Result<Option<ProfileFollow>, KernelError>> + Send;

    /// (follower, followee) の有効な (Pending / Accepted) 世代。
    fn find_active(
        &self,
        executor: &mut Self::Connection,
        follower: &ProfileGraphActor,
        followee: &ProfileGraphActor,
    ) -> impl Future<Output = error_stack::Result<Option<ProfileFollow>, KernelError>> + Send;

    /// (follower, followee) の全世代を古い順に返す (終了済みを含む)。
    fn list_generations(
        &self,
        executor: &mut Self::Connection,
        follower: &ProfileGraphActor,
        followee: &ProfileGraphActor,
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileFollow>, KernelError>> + Send;

    /// `profile` が follower の有効な世代 (following 一覧)。ID 昇順、`after` の次から `limit` 件。
    fn list_following(
        &self,
        executor: &mut Self::Connection,
        profile: &ProfileId,
        after: Option<&ProfileFollowId>,
        limit: i64,
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileFollow>, KernelError>> + Send;

    /// `profile` が followee の有効な世代 (followers 一覧)。ID 昇順、`after` の次から `limit` 件。
    fn list_followers(
        &self,
        executor: &mut Self::Connection,
        profile: &ProfileId,
        after: Option<&ProfileFollowId>,
        limit: i64,
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileFollow>, KernelError>> + Send;

    /// Pending → Accepted。Pending でなければ何もせず false。
    fn accept(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileFollowId,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    /// 有効な世代を終了する。既に終了していれば何もせず false。
    fn end(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileFollowId,
        reason: ProfileFollowEndReason,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    /// `a` と `b` の間の有効な世代を両方向とも終了し、終了した世代 (終了後の状態) を返す。
    /// block の「双方向 Follow 解除」で使う。受理済みだったかは `accepted_at` で判別できる。
    fn end_active_between(
        &self,
        executor: &mut Self::Connection,
        a: &ProfileGraphActor,
        b: &ProfileGraphActor,
        reason: ProfileFollowEndReason,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileFollow>, KernelError>> + Send;
}

pub trait DependOnProfileFollowRepository: Sync + Send + DependOnDatabaseConnection {
    type ProfileFollowRepository: ProfileFollowRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_follow_repository(&self) -> &Self::ProfileFollowRepository;
}
