//! 登録ゲートの control plane use case (D15 / E15a)。
//!
//! 管理者向け (admin 判定は Keto の `instance:administrate` を再利用) の取得・開閉・override・
//! 通知一覧/既読と、内部入力 (テスト fixture / 後続 unit) 専用の観測入力 use case。
//! 観測入力は公開 HTTP からは到達できない。

use crate::permission::{check_permission, instance_administrate};
use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::repository::{
    apply_gate_command, apply_gate_observation, DependOnRegistrationGateRepository,
    RegistrationGateRepository,
};
use kernel::prelude::entity::{
    evaluate, AuthAccountId, GateCommand, RegistrationDecision, RegistrationGateEventId,
    RegistrationGateNotification, RegistrationGateState, RegistrationObservation,
};
use kernel::KernelError;
use std::future::Future;
use time::OffsetDateTime;

pub const DEFAULT_NOTIFICATION_LIMIT: i64 = 50;
pub const MAX_NOTIFICATION_LIMIT: i64 = 100;
pub const MAX_MARK_READ_IDS: usize = 100;

/// 状態とその判定結果。管理画面の常時警告表示 (F15) は `state.override_info` を参照する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationGateView {
    pub state: RegistrationGateState,
    pub decision: RegistrationDecision,
}

impl RegistrationGateView {
    pub fn new(state: RegistrationGateState) -> Self {
        let decision = evaluate(&state);
        Self { state, decision }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationGateNotifications {
    pub notifications: Vec<RegistrationGateNotification>,
    /// 呼び出した管理者自身の未読件数。
    pub unread_count: i64,
    /// 次ページ取得用 (`before` に渡す)。最後のページでは None。
    pub next_cursor: Option<RegistrationGateEventId>,
}

/// PostgreSQL の timestamptz に合わせてマイクロ秒へ丸める (保存前後で値が一致する)。
fn now() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    now.replace_nanosecond(now.nanosecond() / 1000 * 1000)
        .unwrap_or(now)
}

async fn run_command<T>(
    deps: &T,
    admin: &AuthAccountId,
    revision: i64,
    command: GateCommand,
) -> error_stack::Result<RegistrationGateView, KernelError>
where
    T: 'static
        + Clone
        + DependOnRegistrationGateRepository
        + DependOnTransactionManager
        + DependOnPermissionChecker,
{
    check_permission(deps, admin, &instance_administrate()).await?;
    let transaction_deps = deps.clone();
    let actor = admin.clone();
    let state = deps
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                apply_gate_command(
                    transaction_deps.registration_gate_repository(),
                    executor,
                    revision,
                    command,
                    &actor,
                    now(),
                )
                .await
            })
        })
        .await?;
    Ok(RegistrationGateView::new(state))
}

pub trait GetRegistrationGateUseCase:
    'static + Sync + Send + DependOnRegistrationGateRepository + DependOnPermissionChecker
{
    fn get_registration_gate<'a>(
        &'a self,
        admin: &'a AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<RegistrationGateView, KernelError>> + Send + 'a
    {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let mut connection = self.database_connection().connection().await?;
            let state = self
                .registration_gate_repository()
                .load(&mut connection)
                .await?;
            Ok(RegistrationGateView::new(state))
        }
    }
}

impl<T> GetRegistrationGateUseCase for T where
    T: 'static + Sync + Send + DependOnRegistrationGateRepository + DependOnPermissionChecker
{
}

/// 管理者の開閉・手動再開・override。すべて revision 必須で、状態・監査・通知イベントを
/// 同一 transaction で保存する。
pub trait ChangeRegistrationGateUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnRegistrationGateRepository
    + DependOnTransactionManager
    + DependOnPermissionChecker
{
    fn open_registration_gate<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        revision: i64,
    ) -> impl Future<Output = error_stack::Result<RegistrationGateView, KernelError>> + Send + 'a
    {
        run_command(self, admin, revision, GateCommand::Open)
    }

    fn close_registration_gate<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        revision: i64,
    ) -> impl Future<Output = error_stack::Result<RegistrationGateView, KernelError>> + Send + 'a
    {
        run_command(self, admin, revision, GateCommand::Close)
    }

    fn enable_registration_gate_override<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        revision: i64,
        warning_acknowledged: bool,
    ) -> impl Future<Output = error_stack::Result<RegistrationGateView, KernelError>> + Send + 'a
    {
        run_command(
            self,
            admin,
            revision,
            GateCommand::EnableOverride {
                warning_acknowledged,
            },
        )
    }

    fn revoke_registration_gate_override<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        revision: i64,
    ) -> impl Future<Output = error_stack::Result<RegistrationGateView, KernelError>> + Send + 'a
    {
        run_command(self, admin, revision, GateCommand::RevokeOverride)
    }
}

impl<T> ChangeRegistrationGateUseCase for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnRegistrationGateRepository
        + DependOnTransactionManager
        + DependOnPermissionChecker
{
}

/// 通知一覧・未読件数・既読化。既読は呼び出した管理者ごとに独立している。
pub trait RegistrationGateNotificationUseCase:
    'static + Sync + Send + DependOnRegistrationGateRepository + DependOnPermissionChecker
{
    fn list_registration_gate_notifications<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        before: Option<RegistrationGateEventId>,
        limit: Option<i64>,
    ) -> impl Future<Output = error_stack::Result<RegistrationGateNotifications, KernelError>> + Send + 'a
    {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let limit = limit
                .unwrap_or(DEFAULT_NOTIFICATION_LIMIT)
                .clamp(1, MAX_NOTIFICATION_LIMIT);
            let mut connection = self.database_connection().connection().await?;
            let repository = self.registration_gate_repository();
            let notifications = repository
                .list_notifications(&mut connection, admin, before.as_ref(), limit)
                .await?;
            let unread_count = repository.count_unread(&mut connection, admin).await?;
            let next_cursor = (notifications.len() as i64 >= limit)
                .then(|| notifications.last().map(|n| n.event.id.clone()))
                .flatten();
            Ok(RegistrationGateNotifications {
                notifications,
                unread_count,
                next_cursor,
            })
        }
    }

    fn count_unread_registration_gate_notifications<'a>(
        &'a self,
        admin: &'a AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<i64, KernelError>> + Send + 'a {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let mut connection = self.database_connection().connection().await?;
            self.registration_gate_repository()
                .count_unread(&mut connection, admin)
                .await
        }
    }

    /// 指定イベントを呼び出した管理者について既読にし、そのあとの未読件数を返す。
    fn mark_registration_gate_notifications_read<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        event_ids: Vec<RegistrationGateEventId>,
    ) -> impl Future<Output = error_stack::Result<i64, KernelError>> + Send + 'a {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            if event_ids.is_empty() || event_ids.len() > MAX_MARK_READ_IDS {
                return Err(
                    Report::new(KernelError::Validation).attach_printable(format!(
                        "event_ids must contain between 1 and {MAX_MARK_READ_IDS} ids"
                    )),
                );
            }
            let mut connection = self.database_connection().connection().await?;
            let repository = self.registration_gate_repository();
            repository
                .mark_read(&mut connection, admin, &event_ids, now())
                .await?;
            repository.count_unread(&mut connection, admin).await
        }
    }

    /// 全イベントを呼び出した管理者について既読にし、そのあとの未読件数を返す。
    fn mark_all_registration_gate_notifications_read<'a>(
        &'a self,
        admin: &'a AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<i64, KernelError>> + Send + 'a {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let mut connection = self.database_connection().connection().await?;
            let repository = self.registration_gate_repository();
            repository
                .mark_all_read(&mut connection, admin, now())
                .await?;
            repository.count_unread(&mut connection, admin).await
        }
    }
}

impl<T> RegistrationGateNotificationUseCase for T where
    T: 'static + Sync + Send + DependOnRegistrationGateRepository + DependOnPermissionChecker
{
}

/// 観測入力 (設定有無 + 検証 API の観測結果) の反映。公開 HTTP の書込口は作らず、
/// テスト fixture と後続 unit (E15c/E15d) の内部 use case からのみ呼ぶ。
/// 制御状態が変わらない反復では revision・監査・通知を増やさない。
pub trait RecordRegistrationObservationUseCase:
    'static + Sync + Send + Clone + DependOnRegistrationGateRepository + DependOnTransactionManager
{
    fn record_registration_observation(
        &self,
        observation: RegistrationObservation,
    ) -> impl Future<Output = error_stack::Result<RegistrationGateView, KernelError>> + Send {
        async move {
            let transaction_deps = self.clone();
            let state = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        apply_gate_observation(
                            transaction_deps.registration_gate_repository(),
                            executor,
                            observation,
                            now(),
                        )
                        .await
                    })
                })
                .await?;
            Ok(RegistrationGateView::new(state))
        }
    }
}

impl<T> RecordRegistrationObservationUseCase for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnRegistrationGateRepository
        + DependOnTransactionManager
{
}

#[cfg(test)]
mod tests;
