use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    GraphActivityId, GraphActorUrl, InboundActivityReceipt, InboundActivityRecord,
    NewInboundActivity, ProfileId,
};
use crate::KernelError;
use std::future::Future;
use time::OffsetDateTime;

/// 受信 activity の重複検知記録の保存 port。
pub trait InboundActivityRepository: Sync + Send + 'static {
    type Connection: Connection;

    /// 受信を記録する。初めてなら `first_seen = true`。既にあれば受信回数と最終受信時刻だけ更新し
    /// `first_seen = false` を返す (同時受信でも片方だけが `true`)。
    ///
    /// 処理の失敗で再処理を許したい場合は、activity の処理と同じ transaction で呼ぶこと
    /// (rollback で記録も戻る)。
    fn record(
        &self,
        executor: &mut Self::Connection,
        activity: &NewInboundActivity,
    ) -> impl Future<Output = error_stack::Result<InboundActivityReceipt, KernelError>> + Send;

    fn find(
        &self,
        executor: &mut Self::Connection,
        receiver: &ProfileId,
        actor: &GraphActorUrl,
        activity_id: &GraphActivityId,
    ) -> impl Future<Output = error_stack::Result<Option<InboundActivityRecord>, KernelError>> + Send;

    /// `cutoff` より前に初めて受信した記録を消す (保持期間の管理用)。消した件数を返す。
    fn purge_received_before(
        &self,
        executor: &mut Self::Connection,
        cutoff: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<u64, KernelError>> + Send;
}

pub trait DependOnInboundActivityRepository: Sync + Send + DependOnDatabaseConnection {
    type InboundActivityRepository: InboundActivityRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn inbound_activity_repository(&self) -> &Self::InboundActivityRepository;
}
