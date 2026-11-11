use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    CancelledProfileDeliveryJob, ClaimedProfileDeliveryJob, DeliveryActivityType,
    DeliveryAttemptOutcome, DeliveryClaimPolicy, EnqueueOutcome, GraphActivityId, GraphInboxUrl,
    NewProfileDeliveryJob, ProfileDeliveryAttempt, ProfileDeliveryJob, ProfileDeliveryJobId,
    ProfileFollowId, ProfileId, RecordAttemptOutcome,
};
use crate::KernelError;
use std::future::Future;
use time::OffsetDateTime;

/// 配送 job の予約 port。**P3o (送信側) と P3i (受信側) が共通に使う enqueue interface。**
///
/// 関係の変更 (`ProfileFollowRepository` など) と同じ transaction で予約できるよう、
/// 他の repository と同様に呼び出し側の `executor` (transaction 接続) 上で動く:
///
/// ```ignore
/// database.transaction(|conn| Box::pin(async move {
///     follows.begin_generation(conn, &follow).await?;
///     enqueuer.enqueue(conn, &job).await?;   // 同じ transaction。片方だけ commit されない
///     Ok(())
/// })).await
/// ```
///
/// 保存層は配送しない。署名・HTTP 送信・バックオフは worker (P3o) が `ProfileDeliveryJobRepository`
/// を通して行う。
pub trait ProfileDeliveryEnqueuer: Sync + Send + 'static {
    type Connection: Connection;

    /// `(sender, activity_id, target_inbox)` で冪等。既に同じ組の job があれば何も変更せず
    /// `AlreadyEnqueued` を返す (状態は問わない。終端状態の job も再作成しない)。
    fn enqueue(
        &self,
        executor: &mut Self::Connection,
        job: &NewProfileDeliveryJob,
    ) -> impl Future<Output = error_stack::Result<EnqueueOutcome, KernelError>> + Send;
}

pub trait DependOnProfileDeliveryEnqueuer: Sync + Send + DependOnDatabaseConnection {
    type ProfileDeliveryEnqueuer: ProfileDeliveryEnqueuer<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_delivery_enqueuer(&self) -> &Self::ProfileDeliveryEnqueuer;
}

/// 配送 worker (P3o) 向けの job の取得・結果記録・取消・参照 port。
pub trait ProfileDeliveryJobRepository: Sync + Send + 'static {
    type Connection: Connection;

    /// 試行可能な job (Pending / Retrying で `next_attempt_at <= now`、および lease 期限切れの InFlight)
    /// を最大 `policy.limit` 件、lease つきで取得する。複数 worker が同時に呼んでも同じ job は
    /// 二重に取得されない (`FOR UPDATE SKIP LOCKED`)。
    ///
    /// 期限切れ lease の回収では、前回の試行を `LeaseExpired` として履歴に残す。
    /// `policy.max_attempts` に達していた job は再取得せず `Failed(RetriesExhausted)` にする。
    fn claim_due(
        &self,
        executor: &mut Self::Connection,
        worker_id: &str,
        now: OffsetDateTime,
        policy: &DeliveryClaimPolicy,
    ) -> impl Future<Output = error_stack::Result<Vec<ClaimedProfileDeliveryJob>, KernelError>> + Send;

    /// 送信結果を記録する。lease を保持していて job が取り消されていなければ `Recorded`。
    fn record_attempt(
        &self,
        executor: &mut Self::Connection,
        claim: &ClaimedProfileDeliveryJob,
        now: OffsetDateTime,
        outcome: &DeliveryAttemptOutcome,
    ) -> impl Future<Output = error_stack::Result<RecordAttemptOutcome, KernelError>> + Send;

    /// 送信の直前に lease を確認して延長する (P3o が追加した操作。P2 の取得・記録の契約は変えない)。
    ///
    /// `claim` の lease をまだ保持している (job が `InFlight` で `lease_token` が一致し、試行番号が同じで、
    /// 期限が `max(now, DB の現在時刻)` より後、取消されていない) ときだけ `lease_expires_at` を
    /// `max(now, DB の現在時刻) + extension` にして true を返す (行ロックの待機で `now` が古くなっても、期限判定と
    /// 延長は更新を実行した時刻を基準にする)。取消・期限切れ・他 worker による回収があれば何も変えず false (呼び出し側は送信しない)。
    /// 鍵の取得・署名などで claim から時間が経った worker が、lease を失ったまま送信するのを防ぐ。
    fn renew_lease(
        &self,
        executor: &mut Self::Connection,
        claim: &ClaimedProfileDeliveryJob,
        now: OffsetDateTime,
        extension: time::Duration,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    /// 未配送 (Pending / InFlight / Retrying) の job を取り消す。配送済み・失敗・取消済みなら false。
    /// InFlight の job を取り消しても、既に送信された activity は取り戻せない
    /// (worker は `record_attempt` で `Cancelled` を受け取る)。
    fn cancel(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileDeliveryJobId,
        reason: &str,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    /// `cancel` と同じ取消を行い、取り消した job の取消直前の状態 (状態・試行回数・lease の期限) を返す。job の行を
    /// ロックして読み直してから取り消すので、読取後に claim / renew で状態や期限が変わっても、返す値は取消の
    /// 瞬間のもの。取り消せなかった (配送済み・失敗・取消済み) ときは `None`。
    fn cancel_returning(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileDeliveryJobId,
        reason: &str,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<Option<CancelledProfileDeliveryJob>, KernelError>> + Send;

    /// `cancel_undelivered_for_follow` と同じ取消を行い、取り消した各 job の取消直前の状態を返す
    /// (`cancel_returning` と同じ規則)。
    fn cancel_undelivered_for_follow_returning(
        &self,
        executor: &mut Self::Connection,
        follow_id: &ProfileFollowId,
        activity_type: DeliveryActivityType,
        reason: &str,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<Vec<CancelledProfileDeliveryJob>, KernelError>> + Send;

    /// 個別 block の actor 対 (送信元 x 相手 actor) の操作を直列化する transaction 内の排他ロック
    /// (transaction 終了で解放)。block / unblock は、この actor 対の job と下限を読み書きする前に取る。
    fn lock_block_pair(
        &self,
        executor: &mut Self::Connection,
        sender: &ProfileId,
        target_actor_url: &str,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    /// actor 対の「Block / Undo(Block) をこれより前に送らない」下限 (無ければ `None`)。
    fn find_block_settle(
        &self,
        executor: &mut Self::Connection,
        sender: &ProfileId,
        target_actor_url: &str,
    ) -> impl Future<Output = error_stack::Result<Option<OffsetDateTime>, KernelError>> + Send;

    /// 下限を `not_before` まで引き上げる (既存値との `GREATEST`。単調非減少で、短縮されない)。更新後の値を返す。
    fn raise_block_settle(
        &self,
        executor: &mut Self::Connection,
        sender: &ProfileId,
        target_actor_url: &str,
        not_before: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<OffsetDateTime, KernelError>> + Send;

    /// `follow_id` に紐づく未配送の `activity_type` の job をまとめて取り消す。取り消した件数を返す。
    /// 例: unfollow 時に未送信の Follow job を取り消す。
    fn cancel_undelivered_for_follow(
        &self,
        executor: &mut Self::Connection,
        follow_id: &ProfileFollowId,
        activity_type: DeliveryActivityType,
        reason: &str,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<u64, KernelError>> + Send;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileDeliveryJobId,
    ) -> impl Future<Output = error_stack::Result<Option<ProfileDeliveryJob>, KernelError>> + Send;

    fn find_by_key(
        &self,
        executor: &mut Self::Connection,
        sender: &ProfileId,
        activity_id: &GraphActivityId,
        target_inbox: &GraphInboxUrl,
    ) -> impl Future<Output = error_stack::Result<Option<ProfileDeliveryJob>, KernelError>> + Send;

    /// 送信元と activity ID が一致する job を古い順に返す (宛先 inbox・状態は問わない)。個別 block の `Block` job を
    /// block から引く (activity ID が block ID から決まる) ために使う。
    fn find_by_activity(
        &self,
        executor: &mut Self::Connection,
        sender: &ProfileId,
        activity_id: &GraphActivityId,
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileDeliveryJob>, KernelError>> + Send;

    /// 送信元の、ある相手 actor (`target_actor_url`) との個別 block に関する job (`Block` と、`Block` を埋め込んだ
    /// `Undo`) を ID 昇順 (作成順) で返す。状態は問わない。block / unblock を繰り返したとき、相手側から見て
    /// 最新の意思が勝つよう、同じ actor 対の未配送 job を取り消し、相手側に残り得る block を判断するために使う。
    fn list_block_pair_jobs(
        &self,
        executor: &mut Self::Connection,
        sender: &ProfileId,
        target_actor_url: &str,
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileDeliveryJob>, KernelError>> + Send;

    /// `follow_id` に紐づく `activity_type` の job を古い順に返す (状態は問わない。終端状態を含む)。
    /// 関係の終了時に、既に予約済みの Follow / Accept job の宛先 inbox と状態を知るために使う
    /// (P3o が追加した読み取り専用の参照。P2 の保存・予約の契約は変えない)。
    fn find_by_follow(
        &self,
        executor: &mut Self::Connection,
        follow_id: &ProfileFollowId,
        activity_type: DeliveryActivityType,
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileDeliveryJob>, KernelError>> + Send;

    /// 試行履歴 (古い順)。
    fn list_attempts(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileDeliveryJobId,
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileDeliveryAttempt>, KernelError>> + Send;
}

pub trait DependOnProfileDeliveryJobRepository: Sync + Send + DependOnDatabaseConnection {
    type ProfileDeliveryJobRepository: ProfileDeliveryJobRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_delivery_job_repository(&self) -> &Self::ProfileDeliveryJobRepository;
}
