use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    apply_command, apply_observation, AuthAccountId, GateCommand, GateTransitionRecord,
    RegistrationGateAudit, RegistrationGateEventId, RegistrationGateNotification,
    RegistrationGateState, RegistrationObservation,
};
use crate::KernelError;
use std::future::Future;
use time::OffsetDateTime;

/// 登録ゲートの永続化ポート (singleton 状態 + 監査 + 通知イベント + 管理者別既読)。
///
/// 状態・監査・通知イベントの書込みは呼び出し側が開いた transaction の `executor` に対して行う。
/// 同一 transaction で保存することが契約で、`apply_gate_command` / `apply_gate_observation` を
/// `TransactionManager::transaction` の中で呼ぶことで満たす。
pub trait RegistrationGateRepository: Sync + Send + 'static {
    type Connection: Connection;

    /// 状態を読み出す。行がなければ初期状態 (Closed, revision 0) を作る。
    fn load(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<RegistrationGateState, KernelError>> + Send;

    /// 状態を行ロック付きで読み出す。書込み前の読出しはこちらを使う。
    fn load_for_update(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<RegistrationGateState, KernelError>> + Send;

    /// 新状態・監査・通知イベントを保存する。`expected_revision` と現在の revision が違えば
    /// `KernelError::Concurrency`。
    fn save_transition(
        &self,
        executor: &mut Self::Connection,
        record: &GateTransitionRecord,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    /// 制御状態が変わらない観測値のみの更新 (revision・監査・通知は増やさない)。
    fn save_observation(
        &self,
        executor: &mut Self::Connection,
        observation: &RegistrationObservation,
        updated_at: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    /// 新しい順の監査記録。
    fn list_audit(
        &self,
        executor: &mut Self::Connection,
        limit: i64,
    ) -> impl Future<Output = error_stack::Result<Vec<RegistrationGateAudit>, KernelError>> + Send;

    /// 新しい順の通知一覧。`read_at` は `admin` 自身の既読のみを反映する。
    /// `before` は直前ページ最後の event id (それより古いものを返す)。
    fn list_notifications(
        &self,
        executor: &mut Self::Connection,
        admin: &AuthAccountId,
        before: Option<&RegistrationGateEventId>,
        limit: i64,
    ) -> impl Future<Output = error_stack::Result<Vec<RegistrationGateNotification>, KernelError>> + Send;

    /// `admin` の未読件数 (他の管理者の既読は影響しない)。
    fn count_unread(
        &self,
        executor: &mut Self::Connection,
        admin: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<i64, KernelError>> + Send;

    /// 指定イベントを `admin` について既読にする。新たに既読になった件数を返す。
    /// 存在しない event id は無視する。
    fn mark_read(
        &self,
        executor: &mut Self::Connection,
        admin: &AuthAccountId,
        event_ids: &[RegistrationGateEventId],
        read_at: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<u64, KernelError>> + Send;

    /// 全イベントを `admin` について既読にする。新たに既読になった件数を返す。
    fn mark_all_read(
        &self,
        executor: &mut Self::Connection,
        admin: &AuthAccountId,
        read_at: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<u64, KernelError>> + Send;
}

pub trait DependOnRegistrationGateRepository: Sync + Send + DependOnDatabaseConnection {
    type RegistrationGateRepository: RegistrationGateRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn registration_gate_repository(&self) -> &Self::RegistrationGateRepository;
}

/// 管理者操作を適用して保存する。transaction 内で呼ぶこと。
pub async fn apply_gate_command<R: RegistrationGateRepository>(
    repository: &R,
    executor: &mut R::Connection,
    expected_revision: i64,
    command: GateCommand,
    actor: &AuthAccountId,
    now: OffsetDateTime,
) -> error_stack::Result<RegistrationGateState, KernelError> {
    let state = repository.load_for_update(executor).await?;
    let transition = apply_command(&state, expected_revision, command, actor, now)?;
    let record = transition.into_record(now);
    repository.save_transition(executor, &record).await?;
    Ok(record.state)
}

/// 観測入力を反映して保存する。transaction 内で呼ぶこと。
/// 制御状態が変わらない反復では revision・監査・通知を増やさない。
pub async fn apply_gate_observation<R: RegistrationGateRepository>(
    repository: &R,
    executor: &mut R::Connection,
    observation: RegistrationObservation,
    now: OffsetDateTime,
) -> error_stack::Result<RegistrationGateState, KernelError> {
    let state = repository.load_for_update(executor).await?;
    let outcome = apply_observation(&state, observation, now);
    match outcome.transition {
        Some(transition) => {
            let record = transition.into_record(now);
            repository.save_transition(executor, &record).await?;
            Ok(record.state)
        }
        None => {
            if outcome.observation_changed {
                repository
                    .save_observation(executor, &observation, now)
                    .await?;
            }
            Ok(outcome.state)
        }
    }
}
