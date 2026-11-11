//! 登録ゲート通知の外部 Webhook 送信先管理と配送の use case (D15 / E15w / E15x)。
//!
//! 管理者向け (admin 判定は E15a と同じ Keto の `instance:administrate`) の送信先 CRUD・テスト送信・
//! failed の再試行・failed 一覧と、配送 worker 用の配送サイクル・環境変数の取り込み。
//! 配送と送信先の変更はゲート状態の transaction の外側で動き、ゲート状態・通知イベント・既読を
//! 変更せず、新しい通知イベント (= 配送対象) を生まない。URL は部分マスク形でしか返さず、secret は
//! 書き込み専用で暗号化して保存する。

use crate::permission::{check_permission, instance_administrate};
use error_stack::Report;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::repository::{
    run_webhook_delivery_cycle, send_webhook_test, DependOnWebhookDeliveryRepository,
    DependOnWebhookSecretCipher, DependOnWebhookSender, WebhookCycleEnv, WebhookCycleReport,
    WebhookDeliveryRepository, WebhookSecretCipher, WebhookSecretError, WebhookSender,
    WebhookTestOutcome,
};
use kernel::prelude::entity::{
    build_failed_page, endpoint_health, mask_webhook_url, AuthAccountId, NewWebhookEndpoint,
    RegistrationGateEventId, StoredWebhookEndpoint, ValidatedWebhookUrl, WebhookAuditEntry,
    WebhookAuditOperation, WebhookAuditOutcome, WebhookCategories, WebhookCategory,
    WebhookConfigStatus, WebhookDeliveryPolicy, WebhookDestination, WebhookEndpointId,
    WebhookEndpointPatch, WebhookEndpointRejection, WebhookEndpointSource, WebhookEndpointStats,
    WebhookEndpointView, WebhookEndpointWrite, WebhookFailedPage, WebhookFailingReason,
    WebhookFailureCursor, WebhookResultClass, WriteOnlySecret, DEFAULT_FAILED_PAGE_LIMIT,
    MAX_FAILED_PAGE_LIMIT, MAX_WEBHOOK_ENDPOINTS,
};
use kernel::KernelError;
use std::future::Future;
use time::OffsetDateTime;

/// 管理者に見せる送信先の一覧と全体の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookEndpointList {
    pub endpoints: Vec<WebhookEndpointView>,
    /// どこかの送信先に未解決の failed 配送がある (管理画面のタブの印用)。
    pub has_failed_deliveries: bool,
    /// 暗号鍵がサーバー設定にあるか (なければ送信先の作成・secret 更新はできない)。
    pub encryption_key_configured: bool,
    pub max_endpoints: i64,
}

/// 送信先の作成入力。
#[derive(Debug)]
pub struct CreateWebhookEndpointInput {
    pub url: String,
    pub secret: WriteOnlySecret,
    pub categories: Vec<WebhookCategory>,
}

/// 送信先の更新入力。`None` は「変更しない」。URL・secret は応答に返らないので、変えるときだけ指定する。
#[derive(Debug)]
pub struct UpdateWebhookEndpointInput {
    pub revision: i64,
    pub url: Option<String>,
    pub secret: Option<WriteOnlySecret>,
    pub categories: Option<Vec<WebhookCategory>>,
}

/// PostgreSQL の timestamptz に合わせてマイクロ秒へ丸める。
fn now() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    now.replace_nanosecond(now.nanosecond() / 1000 * 1000)
        .unwrap_or(now)
}

/// 拒否理由を `KernelError::Validation` + 固定コードの添付で返す (API は 400/409 に対応付ける)。
pub fn rejection_report(rejection: WebhookEndpointRejection) -> Report<KernelError> {
    Report::new(KernelError::Validation)
        .attach(rejection)
        .attach_printable(rejection.code().to_string())
}

fn secret_error(error: WebhookSecretError) -> WebhookEndpointRejection {
    match error {
        WebhookSecretError::KeyNotConfigured => {
            WebhookEndpointRejection::EncryptionKeyNotConfigured
        }
        WebhookSecretError::Undecryptable => WebhookEndpointRejection::EncryptionKeyNotConfigured,
    }
}

pub trait RegistrationGateWebhookUseCase:
    'static
    + Sized
    + Sync
    + Send
    + DependOnWebhookDeliveryRepository
    + DependOnWebhookSender
    + DependOnWebhookSecretCipher
    + DependOnPermissionChecker
{
    fn list_registration_gate_webhook_endpoints<'a>(
        &'a self,
        admin: &'a AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<WebhookEndpointList, KernelError>> + Send + 'a
    {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let mut connection = self.database_connection().connection().await?;
            let repository = self.webhook_delivery_repository();
            let stored = repository.list_endpoints(&mut connection).await?;
            let stats = repository.endpoint_stats(&mut connection).await?;
            let imported = repository.environment_import_done(&mut connection).await?;
            let has_failed_deliveries = stats.iter().any(|(_, stats)| stats.counts.failed > 0);
            let mut endpoints = Vec::new();
            if let Some(view) = environment_view(self, imported, &stats) {
                endpoints.push(view);
            }
            endpoints.extend(
                stored
                    .iter()
                    .map(|endpoint| stored_view(self, endpoint, &stats)),
            );
            Ok(WebhookEndpointList {
                endpoints,
                has_failed_deliveries,
                encryption_key_configured: self.webhook_secret_cipher().is_configured(),
                max_endpoints: MAX_WEBHOOK_ENDPOINTS,
            })
        }
    }

    fn get_registration_gate_webhook_endpoint<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        id: WebhookEndpointId,
    ) -> impl Future<Output = error_stack::Result<WebhookEndpointView, KernelError>> + Send + 'a
    {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let mut connection = self.database_connection().connection().await?;
            endpoint_view(self, &mut connection, id).await
        }
    }

    /// 送信先を登録する。URL は登録時の宛先検査 (SSRF 対策) を通す。secret は暗号化して保存する
    /// (暗号鍵がなければ平文保存せず拒否)。作成後に発生したイベントだけを配送する。
    fn create_registration_gate_webhook_endpoint<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        input: CreateWebhookEndpointInput,
    ) -> impl Future<Output = error_stack::Result<WebhookEndpointView, KernelError>> + Send + 'a
    {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let mut connection = self.database_connection().connection().await?;
            let result = async {
                let categories = WebhookCategories::new(input.categories.iter().copied())
                    .ok_or(WebhookEndpointRejection::NoCategories)?;
                let secret = input
                    .secret
                    .parse()
                    .map_err(WebhookEndpointRejection::from)?;
                if !self.webhook_secret_cipher().is_configured() {
                    return Err(WebhookEndpointRejection::EncryptionKeyNotConfigured);
                }
                let url = self
                    .webhook_sender()
                    .validate_destination(&input.url)
                    .await?;
                let id = WebhookEndpointId::generate();
                let sealed = self
                    .webhook_secret_cipher()
                    .seal(id, &secret)
                    .map_err(secret_error)?;
                Ok(NewWebhookEndpoint {
                    id,
                    url,
                    sealed_secret: sealed,
                    categories,
                    source: WebhookEndpointSource::Api,
                })
            }
            .await;
            let new = match result {
                Ok(new) => new,
                Err(rejection) => {
                    audit(
                        self,
                        &mut connection,
                        Some(admin),
                        None,
                        WebhookAuditOperation::Create,
                        WebhookAuditOutcome::Rejected,
                        None,
                        Some(rejection.code()),
                    )
                    .await?;
                    return Err(rejection_report(rejection));
                }
            };
            let outcome = self
                .webhook_delivery_repository()
                .create_endpoint(&mut connection, new, admin, now())
                .await?;
            match outcome {
                WebhookEndpointWrite::Done(stored) => {
                    let stats = self
                        .webhook_delivery_repository()
                        .endpoint_stats(&mut connection)
                        .await?;
                    Ok(stored_view(self, &stored, &stats))
                }
                WebhookEndpointWrite::DuplicateUrl => {
                    reject_after(
                        self,
                        &mut connection,
                        admin,
                        None,
                        WebhookAuditOperation::Create,
                        WebhookEndpointRejection::DuplicateUrl,
                    )
                    .await
                }
                WebhookEndpointWrite::LimitReached => {
                    reject_after(
                        self,
                        &mut connection,
                        admin,
                        None,
                        WebhookAuditOperation::Create,
                        WebhookEndpointRejection::EndpointLimitReached,
                    )
                    .await
                }
                WebhookEndpointWrite::RevisionConflict | WebhookEndpointWrite::NotFound => {
                    Err(Report::new(KernelError::Internal))
                }
            }
        }
    }

    /// 送信先を更新する (revision 楽観ロック。不一致は `Concurrency` = 409)。URL・secret・通知カテゴリは
    /// 指定したものだけを置き換える。
    fn update_registration_gate_webhook_endpoint<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        id: WebhookEndpointId,
        input: UpdateWebhookEndpointInput,
    ) -> impl Future<Output = error_stack::Result<WebhookEndpointView, KernelError>> + Send + 'a
    {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let mut connection = self.database_connection().connection().await?;
            let op = WebhookAuditOperation::Update;
            let prepared = async {
                if id.is_environment() {
                    return Err(WebhookEndpointRejection::EnvironmentEndpointReadOnly);
                }
                if input.url.is_none() && input.secret.is_none() && input.categories.is_none() {
                    return Err(WebhookEndpointRejection::NothingToUpdate);
                }
                let categories = match &input.categories {
                    Some(categories) => Some(
                        WebhookCategories::new(categories.iter().copied())
                            .ok_or(WebhookEndpointRejection::NoCategories)?,
                    ),
                    None => None,
                };
                let sealed = match &input.secret {
                    Some(raw) => {
                        let secret = raw.parse().map_err(WebhookEndpointRejection::from)?;
                        if !self.webhook_secret_cipher().is_configured() {
                            return Err(WebhookEndpointRejection::EncryptionKeyNotConfigured);
                        }
                        Some(
                            self.webhook_secret_cipher()
                                .seal(id, &secret)
                                .map_err(secret_error)?,
                        )
                    }
                    None => None,
                };
                let url = match &input.url {
                    Some(raw) => Some(self.webhook_sender().validate_destination(raw).await?),
                    None => None,
                };
                Ok(WebhookEndpointPatch {
                    url,
                    sealed_secret: sealed,
                    categories,
                })
            }
            .await;
            let patch = match prepared {
                Ok(patch) => patch,
                Err(rejection) => {
                    audit(
                        self,
                        &mut connection,
                        Some(admin),
                        Some(id),
                        op,
                        WebhookAuditOutcome::Rejected,
                        Some(input.revision),
                        Some(rejection.code()),
                    )
                    .await?;
                    return Err(rejection_report(rejection));
                }
            };
            let outcome = self
                .webhook_delivery_repository()
                .update_endpoint(&mut connection, id, input.revision, patch, admin, now())
                .await?;
            match outcome {
                WebhookEndpointWrite::Done(stored) => {
                    let stats = self
                        .webhook_delivery_repository()
                        .endpoint_stats(&mut connection)
                        .await?;
                    Ok(stored_view(self, &stored, &stats))
                }
                WebhookEndpointWrite::DuplicateUrl => {
                    reject_after(
                        self,
                        &mut connection,
                        admin,
                        Some(id),
                        op,
                        WebhookEndpointRejection::DuplicateUrl,
                    )
                    .await
                }
                WebhookEndpointWrite::LimitReached => Err(Report::new(KernelError::Internal)),
                WebhookEndpointWrite::RevisionConflict => {
                    audit(
                        self,
                        &mut connection,
                        Some(admin),
                        Some(id),
                        op,
                        WebhookAuditOutcome::RevisionConflict,
                        Some(input.revision),
                        None,
                    )
                    .await?;
                    Err(Report::new(KernelError::Concurrency)
                        .attach_printable("Webhook endpoint revision mismatch"))
                }
                WebhookEndpointWrite::NotFound => {
                    audit(
                        self,
                        &mut connection,
                        Some(admin),
                        Some(id),
                        op,
                        WebhookAuditOutcome::NotFound,
                        Some(input.revision),
                        None,
                    )
                    .await?;
                    Err(Report::new(KernelError::NotFound)
                        .attach_printable("Webhook endpoint not found"))
                }
            }
        }
    }

    /// 送信先を削除する (revision 楽観ロック)。その送信先の未完了配送は配送されなくなる。
    fn delete_registration_gate_webhook_endpoint<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        id: WebhookEndpointId,
        revision: i64,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let mut connection = self.database_connection().connection().await?;
            let op = WebhookAuditOperation::Delete;
            if id.is_environment() {
                let rejection = WebhookEndpointRejection::EnvironmentEndpointReadOnly;
                audit(
                    self,
                    &mut connection,
                    Some(admin),
                    Some(id),
                    op,
                    WebhookAuditOutcome::Rejected,
                    Some(revision),
                    Some(rejection.code()),
                )
                .await?;
                return Err(rejection_report(rejection));
            }
            let outcome = self
                .webhook_delivery_repository()
                .delete_endpoint(&mut connection, id, revision, admin, now())
                .await?;
            match outcome {
                WebhookEndpointWrite::Done(_) => Ok(()),
                WebhookEndpointWrite::RevisionConflict => {
                    audit(
                        self,
                        &mut connection,
                        Some(admin),
                        Some(id),
                        op,
                        WebhookAuditOutcome::RevisionConflict,
                        Some(revision),
                        None,
                    )
                    .await?;
                    Err(Report::new(KernelError::Concurrency)
                        .attach_printable("Webhook endpoint revision mismatch"))
                }
                WebhookEndpointWrite::NotFound => {
                    audit(
                        self,
                        &mut connection,
                        Some(admin),
                        Some(id),
                        op,
                        WebhookAuditOutcome::NotFound,
                        Some(revision),
                        None,
                    )
                    .await?;
                    Err(Report::new(KernelError::NotFound)
                        .attach_printable("Webhook endpoint not found"))
                }
                WebhookEndpointWrite::DuplicateUrl | WebhookEndpointWrite::LimitReached => {
                    Err(Report::new(KernelError::Internal))
                }
            }
        }
    }

    /// 送信先を指定して合成イベント (`webhook_test`) を、その送信先の署名・宛先検査・timeout で 1 回
    /// だけ送る。ゲート状態・通知一覧・未読件数・配送記録・送信先の状態には書き込まない
    /// (監査だけ記録する)。
    fn send_registration_gate_webhook_test<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        id: WebhookEndpointId,
    ) -> impl Future<Output = error_stack::Result<WebhookTestOutcome, KernelError>> + Send + 'a
    {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let mut connection = self.database_connection().connection().await?;
            let destination = if id.is_environment() {
                let imported = self
                    .webhook_delivery_repository()
                    .environment_import_done(&mut connection)
                    .await?;
                if imported {
                    return Err(not_found());
                }
                match self.webhook_sender().status() {
                    WebhookConfigStatus::Disabled => {
                        return Ok(WebhookTestOutcome::NotConfigured);
                    }
                    WebhookConfigStatus::Misconfigured { .. } => {
                        return Ok(WebhookTestOutcome::Misconfigured);
                    }
                    WebhookConfigStatus::Enabled { .. } => {}
                }
                self.webhook_sender().environment_destination()
            } else {
                let stored = self
                    .webhook_delivery_repository()
                    .get_endpoint(&mut connection, id)
                    .await?;
                let Some(stored) = stored else {
                    audit(
                        self,
                        &mut connection,
                        Some(admin),
                        Some(id),
                        WebhookAuditOperation::Test,
                        WebhookAuditOutcome::NotFound,
                        None,
                        None,
                    )
                    .await?;
                    return Err(not_found());
                };
                open_destination(self, &stored)
            };
            let outcome =
                send_webhook_test(self.webhook_sender(), destination.as_ref(), now()).await?;
            let (audit_outcome, detail) = match &outcome {
                WebhookTestOutcome::Sent {
                    delivered: true, ..
                } => (WebhookAuditOutcome::Succeeded, None),
                WebhookTestOutcome::Sent { result, .. } => (
                    WebhookAuditOutcome::DeliveryFailed,
                    Some(result_code(*result)),
                ),
                _ => (WebhookAuditOutcome::Rejected, None),
            };
            audit(
                self,
                &mut connection,
                Some(admin),
                Some(id),
                WebhookAuditOperation::Test,
                audit_outcome,
                None,
                detail.as_deref(),
            )
            .await?;
            Ok(outcome)
        }
    }

    /// failed の配送を再キューする。`endpoint_id` と `event_id` の両方で (送信先, event ID) 1 件、
    /// `endpoint_id` だけでその送信先の failed 全件、どちらもなければ全送信先の failed 全件。
    /// 同じ event ID を再送する。1 件指定で failed でなければ NotFound。再キューした件数を返す。
    fn retry_failed_registration_gate_webhooks<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        endpoint_id: Option<WebhookEndpointId>,
        event_id: Option<RegistrationGateEventId>,
    ) -> impl Future<Output = error_stack::Result<u64, KernelError>> + Send + 'a {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            if event_id.is_some() && endpoint_id.is_none() {
                return Err(Report::new(KernelError::Validation)
                    .attach_printable("event_id requires endpoint_id".to_string()));
            }
            let mut connection = self.database_connection().connection().await?;
            let op = WebhookAuditOperation::Retry;
            if let Some(id) = endpoint_id {
                let known = if id.is_environment() {
                    let imported = self
                        .webhook_delivery_repository()
                        .environment_import_done(&mut connection)
                        .await?;
                    !imported
                        && !matches!(
                            self.webhook_sender().status(),
                            WebhookConfigStatus::Disabled
                        )
                } else {
                    self.webhook_delivery_repository()
                        .get_endpoint(&mut connection, id)
                        .await?
                        .is_some()
                };
                if !known {
                    audit(
                        self,
                        &mut connection,
                        Some(admin),
                        Some(id),
                        op,
                        WebhookAuditOutcome::NotFound,
                        None,
                        None,
                    )
                    .await?;
                    return Err(not_found());
                }
            }
            let requeued = self
                .webhook_delivery_repository()
                .retry_failed(&mut connection, endpoint_id, event_id.as_ref(), now())
                .await?;
            if event_id.is_some() && requeued == 0 {
                audit(
                    self,
                    &mut connection,
                    Some(admin),
                    endpoint_id,
                    op,
                    WebhookAuditOutcome::NotFound,
                    None,
                    None,
                )
                .await?;
                return Err(Report::new(KernelError::NotFound).attach_printable(
                    "No failed webhook delivery with that endpoint and event id",
                ));
            }
            audit(
                self,
                &mut connection,
                Some(admin),
                endpoint_id,
                op,
                WebhookAuditOutcome::Succeeded,
                None,
                Some(&format!("requeued={requeued}")),
            )
            .await?;
            Ok(requeued)
        }
    }

    /// failed の配送を cursor ページングで返す (failed になった時刻の新しい順 + 送信先 ID + event ID で
    /// 決定的)。件数に関わらず古い failed にも到達できる。成功した配送は出さない。
    fn list_failed_registration_gate_webhook_deliveries<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        endpoint_id: Option<WebhookEndpointId>,
        before: Option<WebhookFailureCursor>,
        limit: Option<i64>,
    ) -> impl Future<Output = error_stack::Result<WebhookFailedPage, KernelError>> + Send + 'a {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let limit = limit
                .unwrap_or(DEFAULT_FAILED_PAGE_LIMIT)
                .clamp(1, MAX_FAILED_PAGE_LIMIT);
            let mut connection = self.database_connection().connection().await?;
            let rows = self
                .webhook_delivery_repository()
                .list_failed(&mut connection, endpoint_id, before.as_ref(), limit + 1)
                .await?;
            Ok(build_failed_page(rows, limit))
        }
    }
}

impl<T> RegistrationGateWebhookUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnWebhookDeliveryRepository
        + DependOnWebhookSender
        + DependOnWebhookSecretCipher
        + DependOnPermissionChecker
{
}

fn not_found() -> Report<KernelError> {
    Report::new(KernelError::NotFound).attach_printable("Webhook endpoint not found")
}

fn result_code(result: WebhookResultClass) -> String {
    serde_json::to_value(result)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// 保存済みの送信先から送信先 (URL + 復号済み secret) を作る。復号できなければ `None`。
fn open_destination<T: DependOnWebhookSecretCipher>(
    deps: &T,
    stored: &StoredWebhookEndpoint,
) -> Option<WebhookDestination> {
    deps.webhook_secret_cipher()
        .open(stored.id, &stored.sealed_secret)
        .ok()
        .map(|secret| WebhookDestination {
            url: stored.url.clone(),
            secret,
        })
}

#[allow(clippy::too_many_arguments)]
async fn audit<T: DependOnWebhookDeliveryRepository>(
    deps: &T,
    connection: &mut <<T as kernel::interfaces::database::DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    actor: Option<&AuthAccountId>,
    endpoint_id: Option<WebhookEndpointId>,
    operation: WebhookAuditOperation,
    outcome: WebhookAuditOutcome,
    revision: Option<i64>,
    detail: Option<&str>,
) -> error_stack::Result<(), KernelError> {
    deps.webhook_delivery_repository()
        .record_audit(
            connection,
            &WebhookAuditEntry {
                actor: actor.cloned(),
                endpoint_id,
                operation,
                outcome,
                revision,
                detail: detail.map(str::to_string),
            },
            now(),
        )
        .await
}

async fn reject_after<T: DependOnWebhookDeliveryRepository, V>(
    deps: &T,
    connection: &mut <<T as kernel::interfaces::database::DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    admin: &AuthAccountId,
    endpoint_id: Option<WebhookEndpointId>,
    operation: WebhookAuditOperation,
    rejection: WebhookEndpointRejection,
) -> error_stack::Result<V, KernelError> {
    audit(
        deps,
        connection,
        Some(admin),
        endpoint_id,
        operation,
        WebhookAuditOutcome::Rejected,
        None,
        Some(rejection.code()),
    )
    .await?;
    Err(rejection_report(rejection))
}

fn stats_of(
    stats: &[(WebhookEndpointId, WebhookEndpointStats)],
    id: WebhookEndpointId,
) -> WebhookEndpointStats {
    stats
        .iter()
        .find(|(candidate, _)| *candidate == id)
        .map(|(_, stats)| stats.clone())
        .unwrap_or_default()
}

fn stored_view<T: DependOnWebhookSecretCipher>(
    deps: &T,
    endpoint: &StoredWebhookEndpoint,
    stats: &[(WebhookEndpointId, WebhookEndpointStats)],
) -> WebhookEndpointView {
    let stats = stats_of(stats, endpoint.id);
    let mut reasons = Vec::new();
    if stats.counts.failed > 0 {
        reasons.push(WebhookFailingReason::FailedDeliveries);
    }
    if deps
        .webhook_secret_cipher()
        .open(endpoint.id, &endpoint.sealed_secret)
        .is_err()
    {
        reasons.push(WebhookFailingReason::SecretUndecryptable);
    }
    WebhookEndpointView {
        id: endpoint.id,
        source: endpoint.source,
        masked_url: mask_webhook_url(&endpoint.url),
        categories: endpoint.categories.clone(),
        revision: Some(endpoint.revision),
        health: endpoint_health(&reasons),
        failing_reasons: reasons,
        failed_count: stats.counts.failed,
        summary: stats.into(),
        secret_configured: true,
        config_issue: None,
        created_at: Some(endpoint.created_at),
        updated_at: Some(endpoint.updated_at),
    }
}

/// 環境変数の送信先 (暗号鍵なしで従来どおり配送している 1 件。read-only)。取り込み済み・未設定なら `None`。
fn environment_view<T: DependOnWebhookSender>(
    deps: &T,
    imported: bool,
    stats: &[(WebhookEndpointId, WebhookEndpointStats)],
) -> Option<WebhookEndpointView> {
    if imported {
        return None;
    }
    let status = deps.webhook_sender().status();
    let (masked_url, config_issue) = match &status {
        WebhookConfigStatus::Disabled => return None,
        WebhookConfigStatus::Misconfigured { issue } => ("••••".to_string(), Some(*issue)),
        WebhookConfigStatus::Enabled { host } => (
            deps.webhook_sender()
                .environment_destination()
                .map(|destination| mask_webhook_url(&destination.url))
                .unwrap_or_else(|| host.clone()),
            None,
        ),
    };
    let stats = stats_of(stats, WebhookEndpointId::ENVIRONMENT);
    let mut reasons = Vec::new();
    if config_issue.is_some() {
        reasons.push(WebhookFailingReason::ConfigInvalid);
    }
    if stats.counts.failed > 0 {
        reasons.push(WebhookFailingReason::FailedDeliveries);
    }
    Some(WebhookEndpointView {
        id: WebhookEndpointId::ENVIRONMENT,
        source: WebhookEndpointSource::Environment,
        masked_url,
        categories: WebhookCategories::all(),
        revision: None,
        health: endpoint_health(&reasons),
        failing_reasons: reasons,
        failed_count: stats.counts.failed,
        summary: stats.into(),
        secret_configured: config_issue.is_none(),
        config_issue,
        created_at: None,
        updated_at: None,
    })
}

async fn endpoint_view<T: RegistrationGateWebhookUseCase>(
    deps: &T,
    connection: &mut <<T as kernel::interfaces::database::DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    id: WebhookEndpointId,
) -> error_stack::Result<WebhookEndpointView, KernelError> {
    let repository = deps.webhook_delivery_repository();
    let stats = repository.endpoint_stats(connection).await?;
    if id.is_environment() {
        let imported = repository.environment_import_done(connection).await?;
        return environment_view(deps, imported, &stats).ok_or_else(not_found);
    }
    let stored = repository
        .get_endpoint(connection, id)
        .await?
        .ok_or_else(not_found)?;
    Ok(stored_view(deps, &stored, &stats))
}

/// 配送 worker 用。管理者認可は不要 (公開 HTTP からは到達できない)。
pub trait DeliverRegistrationGateWebhooksUseCase:
    'static
    + Sync
    + Send
    + DependOnWebhookDeliveryRepository
    + DependOnWebhookSender
    + DependOnWebhookSecretCipher
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
                self.webhook_secret_cipher(),
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
    T: 'static
        + Sync
        + Send
        + DependOnWebhookDeliveryRepository
        + DependOnWebhookSender
        + DependOnWebhookSecretCipher
{
}

/// 起動時の環境変数の取り込み (管理者認可は不要。公開 HTTP からは到達できない)。
pub trait ImportEnvironmentWebhookEndpointUseCase:
    'static
    + Sync
    + Send
    + DependOnWebhookDeliveryRepository
    + DependOnWebhookSender
    + DependOnWebhookSecretCipher
{
    /// 暗号鍵が設定され、環境変数の送信先が有効 (設定不備でない) かつ未取り込みなら、初期送信先
    /// (全 4 カテゴリ) として DB へ一度だけ取り込む。E15w のベースラインと既存の配送記録 (未完了・
    /// failed を含む) を引き継ぐ。冪等で、取り込み後に管理者が削除しても再取り込みしない。
    /// 鍵なし・無効・設定不備・取り込み済みなら何もしない (`None`)。
    fn import_environment_webhook_endpoint<'a>(
        &'a self,
    ) -> impl Future<Output = error_stack::Result<Option<WebhookEndpointId>, KernelError>> + Send + 'a
    {
        async move {
            if !self.webhook_secret_cipher().is_configured() {
                return Ok(None);
            }
            if !self.webhook_sender().status().is_enabled() {
                return Ok(None);
            }
            let mut connection = self.database_connection().connection().await?;
            let repository = self.webhook_delivery_repository();
            if repository.environment_import_done(&mut connection).await? {
                return Ok(None);
            }
            let Some(destination) = self.webhook_sender().environment_destination() else {
                return Ok(None);
            };
            let id = WebhookEndpointId::generate();
            let sealed = self
                .webhook_secret_cipher()
                .seal(id, &destination.secret)
                .map_err(|_| Report::new(KernelError::Internal))?;
            let new = NewWebhookEndpoint {
                id,
                url: ValidatedWebhookUrl::new_checked(destination.url.clone()),
                sealed_secret: sealed,
                categories: WebhookCategories::all(),
                source: WebhookEndpointSource::EnvironmentImport,
            };
            Ok(repository
                .import_environment_endpoint(&mut connection, new, now())
                .await?
                .map(|stored| stored.id))
        }
    }
}

impl<T> ImportEnvironmentWebhookEndpointUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnWebhookDeliveryRepository
        + DependOnWebhookSender
        + DependOnWebhookSecretCipher
{
}

#[cfg(test)]
mod tests;
