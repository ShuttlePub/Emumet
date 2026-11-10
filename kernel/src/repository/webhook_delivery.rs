use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    decide_delivery, ClaimedWebhookDelivery, RegistrationGateEventId, WebhookAttempt,
    WebhookConfigStatus, WebhookDecision, WebhookDelivery, WebhookDeliveryPolicy,
    WebhookDeliverySummary, WebhookEventType, WebhookPayload, WebhookResultClass,
    WebhookSendObservation,
};
use crate::KernelError;
use std::future::Future;
use time::OffsetDateTime;

/// Webhook 配送の 1 回の取り込み結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebhookIngestReport {
    /// 配送対象の下限 (これ以下の ID のイベントは配送しない)。
    pub baseline_event_id: i64,
    pub ingested: u64,
}

/// 配送記録の永続化ポート。各メソッドは内部で atomic (必要なら自前で transaction を張る) で、
/// 呼び出し側は plain connection を渡すだけでよい。
///
/// 配送記録はゲートの状態・通知イベント・既読を一切変更しない (読み取るのは通知イベントと監査のみ)。
pub trait WebhookDeliveryRepository: Sync + Send + 'static {
    type Connection: Connection;

    /// 通知イベントを冪等に配送記録へ取り込む。
    ///
    /// 方針: Webhook が最初に有効と観測された時点の最新イベント ID を baseline として一度だけ保存し、
    /// それ以前に発生したイベントは配送しない (有効化の瞬間に過去の障害通知が一斉に届くのを避ける。
    /// 管理画面の通知一覧には残る)。baseline より後のイベントは、worker 停止・無効化の期間をまたいでも
    /// 取りこぼさず取り込む。
    fn ingest_gate_events(
        &self,
        executor: &mut Self::Connection,
        now: OffsetDateTime,
        limit: i64,
    ) -> impl Future<Output = error_stack::Result<WebhookIngestReport, KernelError>> + Send;

    /// 送信期限の来た配送 (pending/retrying) と lease 期限切れの in_flight (再起動後の回収) を
    /// lease 付きで取得する。`FOR UPDATE SKIP LOCKED` により複数 worker が同一 event を同時に
    /// 取得しない。回収時に上限超過なら failed にする。
    fn claim_due(
        &self,
        executor: &mut Self::Connection,
        owner: &str,
        now: OffsetDateTime,
        policy: &WebhookDeliveryPolicy,
    ) -> impl Future<Output = error_stack::Result<Vec<ClaimedWebhookDelivery>, KernelError>> + Send;

    /// 試行結果を記録する。lease を失っていた (他 worker に回収された) 場合は何も変更せず false。
    fn record_attempt(
        &self,
        executor: &mut Self::Connection,
        claim: &ClaimedWebhookDelivery,
        owner: &str,
        finished_at: OffsetDateTime,
        decision: &WebhookDecision,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    /// failed の配送 (`event_id` 指定ならその 1 件、なければ全件) を再キューする。
    /// 同じ event ID を再送する。再キューした件数を返す。
    fn retry_failed(
        &self,
        executor: &mut Self::Connection,
        event_id: Option<&RegistrationGateEventId>,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<u64, KernelError>> + Send;

    fn summary(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<WebhookDeliverySummary, KernelError>> + Send;

    /// 新しい順の配送記録。
    fn list_recent(
        &self,
        executor: &mut Self::Connection,
        limit: i64,
    ) -> impl Future<Output = error_stack::Result<Vec<WebhookDelivery>, KernelError>> + Send;

    /// 古い順の試行履歴。
    fn list_attempts(
        &self,
        executor: &mut Self::Connection,
        event_id: &RegistrationGateEventId,
    ) -> impl Future<Output = error_stack::Result<Vec<WebhookAttempt>, KernelError>> + Send;
}

pub trait DependOnWebhookDeliveryRepository: Sync + Send + DependOnDatabaseConnection {
    type WebhookDeliveryRepository: WebhookDeliveryRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn webhook_delivery_repository(&self) -> &Self::WebhookDeliveryRepository;
}

/// sender が送る 1 件のリクエスト。署名・timestamp・ヘッダーは sender が付ける。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookRequest<'a> {
    pub event_id: &'a str,
    pub event_type: WebhookEventType,
    /// 配送試行番号 (1 始まり)。
    pub attempt: u32,
    pub body: &'a [u8],
}

/// 外部 Webhook への送信ポート。設定の保持・署名・宛先検査・接続固定は実装側の責務。
pub trait WebhookSender: Sync + Send + 'static {
    /// 設定状態。宛先は host のみ。secret は含まない。
    fn status(&self) -> WebhookConfigStatus;

    fn send(
        &self,
        request: &WebhookRequest<'_>,
    ) -> impl Future<Output = WebhookSendObservation> + Send;
}

pub trait DependOnWebhookSender: Sync + Send {
    type WebhookSender: WebhookSender;

    fn webhook_sender(&self) -> &Self::WebhookSender;
}

/// 配送サイクル 1 回の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebhookCycleReport {
    /// Webhook が有効でなく何もしなかった。
    pub skipped: bool,
    pub ingested: u64,
    pub attempted: u32,
    pub delivered: u32,
    pub retrying: u32,
    pub failed: u32,
    /// 送信中に lease を失い結果を記録しなかった件数。
    pub lease_lost: u32,
}

/// 時刻・乱数・停止判定の注入点。
pub struct WebhookCycleEnv<'a> {
    pub clock: &'a (dyn Fn() -> OffsetDateTime + Sync),
    /// `[0, 1)` の一様乱数 (バックオフの jitter)。
    pub jitter: &'a (dyn Fn() -> f64 + Sync),
    /// true なら次の配送に進まず終了する (cooperative shutdown)。
    pub should_stop: &'a (dyn Fn() -> bool + Sync),
}

const INGEST_LIMIT: i64 = 200;

/// 配送サイクル: 取り込み → (1 件ずつ) lease 取得 → 送信 → 結果記録。ゲートの transaction の外で動く。
/// 1 サイクルで最大 `policy.batch_size` 件を処理する。
///
/// 送信中は DB 接続を保持しない。Webhook が無効・設定不備なら何もしない (ゲート動作に影響しない)。
pub async fn run_webhook_delivery_cycle<D, R, S>(
    database: &D,
    repository: &R,
    sender: &S,
    worker_id: &str,
    policy: &WebhookDeliveryPolicy,
    env: &WebhookCycleEnv<'_>,
) -> error_stack::Result<WebhookCycleReport, KernelError>
where
    D: DatabaseConnection,
    R: WebhookDeliveryRepository<Connection = D::Connection>,
    S: WebhookSender,
{
    let mut report = WebhookCycleReport::default();
    if !sender.status().is_enabled() {
        report.skipped = true;
        return Ok(report);
    }

    {
        let mut connection = database.connection().await?;
        let ingest = repository
            .ingest_gate_events(&mut connection, (env.clock)(), INGEST_LIMIT)
            .await?;
        report.ingested = ingest.ingested;
    }

    // lease は送信の直前に 1 件ずつ取る。バッチでまとめて取ると、後ろの配送の lease が前の配送の送信中に
    // 切れ、別 worker が回収して同じ event を同時に送ってしまう。
    let single = WebhookDeliveryPolicy {
        batch_size: 1,
        ..policy.clone()
    };
    for _ in 0..policy.batch_size.max(1) {
        if (env.should_stop)() {
            break;
        }
        let claim = {
            let mut connection = database.connection().await?;
            repository
                .claim_due(&mut connection, worker_id, (env.clock)(), &single)
                .await?
                .into_iter()
                .next()
        };
        let Some(claim) = claim else {
            break;
        };
        let observation = sender
            .send(&WebhookRequest {
                event_id: &crate::entity::gate_event_wire_id(&claim.event_id),
                event_type: claim.kind.into(),
                attempt: claim.attempt,
                body: claim.payload_json.as_bytes(),
            })
            .await;
        let finished_at = (env.clock)();
        let decision = decide_delivery(
            &observation,
            claim.cycle_attempt,
            finished_at - claim.cycle_started_at,
            policy,
            (env.jitter)(),
        );
        let recorded = {
            let mut connection = database.connection().await?;
            repository
                .record_attempt(&mut connection, &claim, worker_id, finished_at, &decision)
                .await?
        };
        report.attempted += 1;
        if !recorded {
            report.lease_lost += 1;
            continue;
        }
        match decision {
            WebhookDecision::Delivered { .. } => report.delivered += 1,
            WebhookDecision::Retry { .. } => report.retrying += 1,
            WebhookDecision::Failed { .. } => report.failed += 1,
        }
    }
    Ok(report)
}

/// テスト送信 (永続化しない 1 回きりの送信) の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookTestOutcome {
    /// Webhook が未設定。送信していない。
    NotConfigured,
    /// 設定不備。送信していない。
    Misconfigured,
    Sent {
        event_id: String,
        delivered: bool,
        result: WebhookResultClass,
        http_status: Option<u16>,
        attempted_at: OffsetDateTime,
    },
}

/// 合成イベント (`webhook_test`) を実配送と同じ経路 (署名・宛先検査・timeout) で 1 回送る。
/// 配送記録・通知イベント・ゲート状態には一切書き込まない。
pub async fn send_webhook_test<S: WebhookSender>(
    sender: &S,
    now: OffsetDateTime,
) -> error_stack::Result<WebhookTestOutcome, KernelError> {
    match sender.status() {
        WebhookConfigStatus::Disabled => return Ok(WebhookTestOutcome::NotConfigured),
        WebhookConfigStatus::Misconfigured { .. } => return Ok(WebhookTestOutcome::Misconfigured),
        WebhookConfigStatus::Enabled { .. } => {}
    }
    let payload = WebhookPayload::test(crate::generate_id(), now);
    let body = payload.to_json()?;
    let observation = sender
        .send(&WebhookRequest {
            event_id: &payload.event_id,
            event_type: WebhookEventType::WebhookTest,
            attempt: 1,
            body: body.as_bytes(),
        })
        .await;
    let (result, http_status) = crate::entity::classify_single_attempt(&observation);
    Ok(WebhookTestOutcome::Sent {
        event_id: payload.event_id,
        delivered: result == WebhookResultClass::Delivered,
        result,
        http_status,
        attempted_at: now,
    })
}
