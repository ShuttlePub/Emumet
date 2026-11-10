//! 登録ゲート通知の外部 Webhook 配送の use case (D15 / E15w)。
//!
//! 管理者向け (admin 判定は E15a と同じ Keto の `instance:administrate`) の状態取得・テスト送信・
//! failed の再試行と、配送 worker 用の配送サイクル。endpoint・secret の書き換え口は持たない
//! (サーバー設定のみ)。配送はゲート状態の transaction の外側で動き、ゲート状態・通知イベント・
//! 既読を変更しない。

use crate::permission::{check_permission, instance_administrate};
use error_stack::Report;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::repository::{
    run_webhook_delivery_cycle, send_webhook_test, DependOnWebhookDeliveryRepository,
    DependOnWebhookSender, WebhookCycleEnv, WebhookCycleReport, WebhookDeliveryRepository,
    WebhookSender, WebhookTestOutcome,
};
use kernel::prelude::entity::{
    AuthAccountId, RegistrationGateEventId, WebhookConfigStatus, WebhookDelivery,
    WebhookDeliveryPolicy, WebhookDeliverySummary,
};
use kernel::KernelError;
use std::future::Future;
use time::OffsetDateTime;

pub const RECENT_DELIVERY_LIMIT: i64 = 50;

/// 管理者に見せる Webhook の状態。secret・宛先 URL (host 以外) は含まない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookAdminStatus {
    pub config: WebhookConfigStatus,
    pub summary: WebhookDeliverySummary,
    pub recent: Vec<WebhookDelivery>,
}

/// PostgreSQL の timestamptz に合わせてマイクロ秒へ丸める。
fn now() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    now.replace_nanosecond(now.nanosecond() / 1000 * 1000)
        .unwrap_or(now)
}

pub trait RegistrationGateWebhookUseCase:
    'static
    + Sync
    + Send
    + DependOnWebhookDeliveryRepository
    + DependOnWebhookSender
    + DependOnPermissionChecker
{
    fn get_registration_gate_webhook_status<'a>(
        &'a self,
        admin: &'a AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<WebhookAdminStatus, KernelError>> + Send + 'a
    {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let config = self.webhook_sender().status();
            let mut connection = self.database_connection().connection().await?;
            let repository = self.webhook_delivery_repository();
            let summary = repository.summary(&mut connection).await?;
            let recent = repository
                .list_recent(&mut connection, RECENT_DELIVERY_LIMIT)
                .await?;
            Ok(WebhookAdminStatus {
                config,
                summary,
                recent,
            })
        }
    }

    /// 合成イベント (`webhook_test`) を実配送と同じ経路で 1 回送る。ゲート状態・通知一覧・未読件数・
    /// 配送記録のどれにも書き込まない。
    fn send_registration_gate_webhook_test<'a>(
        &'a self,
        admin: &'a AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<WebhookTestOutcome, KernelError>> + Send + 'a
    {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            send_webhook_test(self.webhook_sender(), now()).await
        }
    }

    /// failed の配送を再キューする (`event_id` なしなら全 failed)。再キューした件数を返す。
    /// 同じ event ID を再送する。指定した event が failed でなければ NotFound。
    fn retry_failed_registration_gate_webhooks<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        event_id: Option<RegistrationGateEventId>,
    ) -> impl Future<Output = error_stack::Result<u64, KernelError>> + Send + 'a {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let mut connection = self.database_connection().connection().await?;
            let requeued = self
                .webhook_delivery_repository()
                .retry_failed(&mut connection, event_id.as_ref(), now())
                .await?;
            if event_id.is_some() && requeued == 0 {
                return Err(Report::new(KernelError::NotFound)
                    .attach_printable("No failed webhook delivery with that event id"));
            }
            Ok(requeued)
        }
    }
}

impl<T> RegistrationGateWebhookUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnWebhookDeliveryRepository
        + DependOnWebhookSender
        + DependOnPermissionChecker
{
}

/// 配送 worker 用。管理者認可は不要 (公開 HTTP からは到達できない)。
pub trait DeliverRegistrationGateWebhooksUseCase:
    'static + Sync + Send + DependOnWebhookDeliveryRepository + DependOnWebhookSender
{
    fn deliver_registration_gate_webhooks<'a>(
        &'a self,
        worker_id: &'a str,
        policy: &'a WebhookDeliveryPolicy,
        jitter: &'a (dyn Fn() -> f64 + Sync),
        should_stop: &'a (dyn Fn() -> bool + Sync),
    ) -> impl Future<Output = error_stack::Result<WebhookCycleReport, KernelError>> + Send + 'a
    {
        async move {
            let clock = now;
            run_webhook_delivery_cycle(
                self.database_connection(),
                self.webhook_delivery_repository(),
                self.webhook_sender(),
                worker_id,
                policy,
                &WebhookCycleEnv {
                    clock: &clock,
                    jitter,
                    should_stop,
                },
            )
            .await
        }
    }
}

impl<T> DeliverRegistrationGateWebhooksUseCase for T where
    T: 'static + Sync + Send + DependOnWebhookDeliveryRepository + DependOnWebhookSender
{
}

#[cfg(test)]
mod tests;
