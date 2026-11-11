use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    decide_delivery, AuthAccountId, ClaimedWebhookDelivery, NewWebhookEndpoint,
    RegistrationGateEventId, SealedWebhookSecret, StoredWebhookEndpoint, ValidatedWebhookUrl,
    WebhookAttempt, WebhookAuditEntry, WebhookConfigStatus, WebhookDecision, WebhookDelivery,
    WebhookDeliveryPolicy, WebhookDestination, WebhookEndpointId, WebhookEndpointPatch,
    WebhookEndpointRejection, WebhookEndpointStats, WebhookEndpointWrite, WebhookEventType,
    WebhookFailedDelivery, WebhookFailureCursor, WebhookPayload, WebhookResultClass, WebhookSecret,
    WebhookSendObservation,
};
use crate::KernelError;
use std::future::Future;
use time::OffsetDateTime;

/// Webhook 配送の 1 回の取り込み結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebhookIngestReport {
    /// 環境変数の送信先の配送対象の下限 (これ以下の ID のイベントは配送しない)。
    pub baseline_event_id: i64,
    pub ingested: u64,
}

/// 配送記録と送信先の永続化ポート。各メソッドは内部で atomic (必要なら自前で transaction を張る) で、
/// 呼び出し側は plain connection を渡すだけでよい。
///
/// 配送記録・送信先の変更はゲートの状態・通知イベント・既読を一切変更しない (読み取るのは通知
/// イベントと監査のみ)。配送記録は **(送信先, event ID)** 単位で、event ID は送信先をまたいで同一。
pub trait WebhookDeliveryRepository: Sync + Send + 'static {
    type Connection: Connection;

    /// 通知イベントを冪等に送信先ごとの配送記録へ取り込む。
    ///
    /// 方針: 各送信先は自分の `baseline_event_id` より後のイベントで、かつ**選択中のカテゴリ**に属する
    /// ものだけを取り込む (作成前のイベントは遡及しない)。`include_environment` なら環境変数の送信先
    /// (ID 0) も対象にし、E15w と同じく「有効と観測された時点の最新イベント ID」を baseline にする。
    /// baseline より後のイベントは、worker 停止・無効化の期間をまたいでも取りこぼさず取り込む。
    /// `limit` は 1 送信先あたりの上限 (0 は baseline の確定だけ)。
    fn ingest_gate_events(
        &self,
        executor: &mut Self::Connection,
        now: OffsetDateTime,
        limit: i64,
        include_environment: bool,
    ) -> impl Future<Output = error_stack::Result<WebhookIngestReport, KernelError>> + Send;

    /// 指定した送信先の、送信期限の来た配送 (pending/retrying) と lease 期限切れの in_flight
    /// (再起動後の回収) を lease 付きで取得する。`FOR UPDATE SKIP LOCKED` により複数 worker が
    /// 同一配送を同時に取得しない。回収時に上限超過なら failed にする。
    fn claim_due(
        &self,
        executor: &mut Self::Connection,
        endpoint_id: WebhookEndpointId,
        owner: &str,
        now: OffsetDateTime,
        policy: &WebhookDeliveryPolicy,
    ) -> impl Future<Output = error_stack::Result<Vec<ClaimedWebhookDelivery>, KernelError>> + Send;

    /// 試行結果を記録する。lease を失っていた (他 worker に回収された・送信先が削除された) 場合は
    /// 何も変更せず false。
    fn record_attempt(
        &self,
        executor: &mut Self::Connection,
        claim: &ClaimedWebhookDelivery,
        owner: &str,
        finished_at: OffsetDateTime,
        decision: &WebhookDecision,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    /// failed の配送を再キューする。`endpoint_id` と `event_id` の両方で 1 件、`endpoint_id` だけで
    /// その送信先の failed 全件、どちらもなければ全送信先の failed 全件。同じ event ID を再送する。
    /// 再キューした件数を返す。
    fn retry_failed(
        &self,
        executor: &mut Self::Connection,
        endpoint_id: Option<WebhookEndpointId>,
        event_id: Option<&RegistrationGateEventId>,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<u64, KernelError>> + Send;

    /// 送信先ごとの配送集計 (環境変数の送信先は ID 0)。
    fn endpoint_stats(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<
        Output = error_stack::Result<Vec<(WebhookEndpointId, WebhookEndpointStats)>, KernelError>,
    > + Send;

    /// failed の配送を (failed になった時刻, 送信先 ID, event ID) の降順で `limit` 件返す。
    /// `before` を渡すとその位置より古い側から返す。`endpoint_id` で絞り込める。件数に関わらず
    /// 古い failed にも cursor で到達できる。
    fn list_failed(
        &self,
        executor: &mut Self::Connection,
        endpoint_id: Option<WebhookEndpointId>,
        before: Option<&WebhookFailureCursor>,
        limit: i64,
    ) -> impl Future<Output = error_stack::Result<Vec<WebhookFailedDelivery>, KernelError>> + Send;

    /// 配送記録 (診断・テスト用。管理者 API には出さない)。新しい event ID 順。
    fn list_recent(
        &self,
        executor: &mut Self::Connection,
        endpoint_id: Option<WebhookEndpointId>,
        limit: i64,
    ) -> impl Future<Output = error_stack::Result<Vec<WebhookDelivery>, KernelError>> + Send;

    /// 古い順の試行履歴。
    fn list_attempts(
        &self,
        executor: &mut Self::Connection,
        endpoint_id: WebhookEndpointId,
        event_id: &RegistrationGateEventId,
    ) -> impl Future<Output = error_stack::Result<Vec<WebhookAttempt>, KernelError>> + Send;

    // --- 送信先 ---

    fn list_endpoints(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<Vec<StoredWebhookEndpoint>, KernelError>> + Send;

    fn get_endpoint(
        &self,
        executor: &mut Self::Connection,
        id: WebhookEndpointId,
    ) -> impl Future<Output = error_stack::Result<Option<StoredWebhookEndpoint>, KernelError>> + Send;

    /// 送信先を作成する (上限・URL 重複は結果で返す)。ベースラインは作成時点の最新イベント ID。
    /// 監査は同じ transaction で記録する。
    fn create_endpoint(
        &self,
        executor: &mut Self::Connection,
        new: NewWebhookEndpoint,
        actor: &AuthAccountId,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<WebhookEndpointWrite, KernelError>> + Send;

    /// revision 楽観ロック付きの更新。選択カテゴリの変更前に、変更前のカテゴリで未取り込みの
    /// イベントを取り込む (変更は変更後に発生したイベントにだけ効く)。revision は成功のたびに増える。
    fn update_endpoint(
        &self,
        executor: &mut Self::Connection,
        id: WebhookEndpointId,
        expected_revision: i64,
        patch: WebhookEndpointPatch,
        actor: &AuthAccountId,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<WebhookEndpointWrite, KernelError>> + Send;

    /// revision 楽観ロック付きの削除。その送信先の配送記録・試行履歴も消え、未完了配送は配送されない。
    fn delete_endpoint(
        &self,
        executor: &mut Self::Connection,
        id: WebhookEndpointId,
        expected_revision: i64,
        actor: &AuthAccountId,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<WebhookEndpointWrite, KernelError>> + Send;

    /// 監査を 1 件記録する (URL・secret・通知本文は渡せない型)。
    fn record_audit(
        &self,
        executor: &mut Self::Connection,
        entry: &WebhookAuditEntry,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    /// 環境変数の送信先を取り込み済みか。
    fn environment_import_done(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    /// 環境変数の送信先を初期送信先として一度だけ取り込む (冪等)。ベースラインと ID 0 の既存
    /// 配送記録 (未完了・failed を含む) を引き継ぎ、取り込み済みマーカーを同じ transaction で記録する。
    /// 取り込み済みなら `None` (何も変更しない)。取り込み後に管理者が削除しても再取り込みしない。
    fn import_environment_endpoint(
        &self,
        executor: &mut Self::Connection,
        new: NewWebhookEndpoint,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<Option<StoredWebhookEndpoint>, KernelError>> + Send;
}

pub trait DependOnWebhookDeliveryRepository: Sync + Send + DependOnDatabaseConnection {
    type WebhookDeliveryRepository: WebhookDeliveryRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn webhook_delivery_repository(&self) -> &Self::WebhookDeliveryRepository;
}

/// 暗号鍵まわりの失敗。鍵・secret の値を含まない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookSecretError {
    /// 暗号鍵がサーバー設定にない。
    KeyNotConfigured,
    /// 復号できない (鍵の喪失・誤り・変更、暗号文の破損、送信先 ID の不一致)。
    Undecryptable,
}

/// 送信先 secret の暗号化ポート (AEAD。送信先 ID を関連データに束縛する)。鍵はサーバー設定。
pub trait WebhookSecretCipher: Sync + Send + 'static {
    /// 暗号鍵が設定されているか。
    fn is_configured(&self) -> bool;

    fn seal(
        &self,
        endpoint_id: WebhookEndpointId,
        secret: &WebhookSecret,
    ) -> Result<SealedWebhookSecret, WebhookSecretError>;

    fn open(
        &self,
        endpoint_id: WebhookEndpointId,
        sealed: &SealedWebhookSecret,
    ) -> Result<WebhookSecret, WebhookSecretError>;
}

pub trait DependOnWebhookSecretCipher: Sync + Send {
    type WebhookSecretCipher: WebhookSecretCipher;

    fn webhook_secret_cipher(&self) -> &Self::WebhookSecretCipher;
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

/// 外部 Webhook への送信ポート。署名・宛先検査・接続固定は実装側の責務。
pub trait WebhookSender: Sync + Send + 'static {
    /// 環境変数の送信先の設定状態。宛先は host のみ。secret は含まない。
    fn status(&self) -> WebhookConfigStatus;

    /// 環境変数の送信先 (有効なときだけ)。送信・取り込みの入力としてだけ使う。
    fn environment_destination(&self) -> Option<WebhookDestination>;

    /// 登録時の宛先検査: HTTPS 限定・userinfo/fragment 拒否・host の正規化と検査・名前解決結果の
    /// 全アドレス検査。通った URL (正規化済み) を返す。
    fn validate_destination(
        &self,
        raw_url: &str,
    ) -> impl Future<Output = Result<ValidatedWebhookUrl, WebhookEndpointRejection>> + Send;

    /// 指定した送信先へ送る。接続時にも毎回全アドレスを再検査し、検査したアドレスへ接続を固定し、
    /// リダイレクトを追従せず、proxy を使わず、timeout をかけ、応答本文は読み捨てる。
    fn send_to(
        &self,
        destination: &WebhookDestination,
        request: &WebhookRequest<'_>,
    ) -> impl Future<Output = WebhookSendObservation> + Send;

    /// 環境変数の送信先へ送る (E15w の経路)。
    fn send(
        &self,
        request: &WebhookRequest<'_>,
    ) -> impl Future<Output = WebhookSendObservation> + Send {
        async move {
            match self.environment_destination() {
                Some(destination) => self.send_to(&destination, request).await,
                None => WebhookSendObservation::ConfigInvalid,
            }
        }
    }
}

pub trait DependOnWebhookSender: Sync + Send {
    type WebhookSender: WebhookSender;

    fn webhook_sender(&self) -> &Self::WebhookSender;
}

/// 配送サイクル 1 回の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebhookCycleReport {
    /// 配送対象の送信先がなく何もしなかった。
    pub skipped: bool,
    pub ingested: u64,
    pub attempted: u32,
    pub delivered: u32,
    pub retrying: u32,
    pub failed: u32,
    /// 送信中に lease を失い結果を記録しなかった件数。
    pub lease_lost: u32,
}

impl WebhookCycleReport {
    fn absorb(&mut self, other: &WebhookCycleReport) {
        self.attempted += other.attempted;
        self.delivered += other.delivered;
        self.retrying += other.retrying;
        self.failed += other.failed;
        self.lease_lost += other.lease_lost;
    }
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

/// 1 つの送信先の配送を最大 `policy.batch_size` 件処理する (送信先ごとに独立)。
#[allow(clippy::too_many_arguments)]
async fn deliver_endpoint<D, R, S, C>(
    database: &D,
    repository: &R,
    sender: &S,
    cipher: &C,
    endpoint_id: WebhookEndpointId,
    worker_id: &str,
    policy: &WebhookDeliveryPolicy,
    env: &WebhookCycleEnv<'_>,
) -> error_stack::Result<WebhookCycleReport, KernelError>
where
    D: DatabaseConnection,
    R: WebhookDeliveryRepository<Connection = D::Connection>,
    S: WebhookSender,
    C: WebhookSecretCipher,
{
    let mut report = WebhookCycleReport::default();
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
                .claim_due(
                    &mut connection,
                    endpoint_id,
                    worker_id,
                    (env.clock)(),
                    &single,
                )
                .await?
                .into_iter()
                .next()
        };
        let Some(claim) = claim else {
            break;
        };
        let destination = if endpoint_id.is_environment() {
            sender
                .environment_destination()
                .ok_or(WebhookSendObservation::ConfigInvalid)
        } else {
            let stored = {
                let mut connection = database.connection().await?;
                repository
                    .get_endpoint(&mut connection, endpoint_id)
                    .await?
            };
            match stored {
                // 送信中に削除された。配送記録も消えているので何も記録しない。
                None => break,
                Some(stored) => cipher
                    .open(endpoint_id, &stored.sealed_secret)
                    .map(|secret| WebhookDestination {
                        url: stored.url.clone(),
                        secret,
                    })
                    .map_err(|_| WebhookSendObservation::SecretUnavailable),
            }
        };
        let observation = match destination {
            Ok(destination) => {
                sender
                    .send_to(
                        &destination,
                        &WebhookRequest {
                            event_id: &crate::entity::gate_event_wire_id(&claim.event_id),
                            event_type: claim.kind.into(),
                            attempt: claim.attempt,
                            body: claim.payload_json.as_bytes(),
                        },
                    )
                    .await
            }
            Err(observation) => observation,
        };
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

/// 配送サイクル: 取り込み → 送信先ごとに (1 件ずつ) lease 取得 → 送信 → 結果記録。ゲートの
/// transaction の外で動く。送信先は並行に処理し、ある送信先の遅延・失敗・復号不可は他の送信先を
/// 止めない。1 サイクルで 1 送信先あたり最大 `policy.batch_size` 件を処理する。
///
/// 送信中は DB 接続を保持しない。送信先がなければ何もしない (ゲート動作に影響しない)。
pub async fn run_webhook_delivery_cycle<D, R, S, C>(
    database: &D,
    repository: &R,
    sender: &S,
    cipher: &C,
    worker_id: &str,
    policy: &WebhookDeliveryPolicy,
    env: &WebhookCycleEnv<'_>,
) -> error_stack::Result<WebhookCycleReport, KernelError>
where
    D: DatabaseConnection,
    R: WebhookDeliveryRepository<Connection = D::Connection>,
    S: WebhookSender,
    C: WebhookSecretCipher,
{
    let mut report = WebhookCycleReport::default();
    let endpoint_ids = {
        let mut connection = database.connection().await?;
        let imported = repository.environment_import_done(&mut connection).await?;
        let include_environment = !imported && sender.status().is_enabled();
        let ingest = repository
            .ingest_gate_events(
                &mut connection,
                (env.clock)(),
                INGEST_LIMIT,
                include_environment,
            )
            .await?;
        report.ingested = ingest.ingested;
        let mut ids: Vec<WebhookEndpointId> = Vec::new();
        if include_environment {
            ids.push(WebhookEndpointId::ENVIRONMENT);
        }
        ids.extend(
            repository
                .list_endpoints(&mut connection)
                .await?
                .into_iter()
                .map(|endpoint| endpoint.id),
        );
        ids
    };
    if endpoint_ids.is_empty() {
        report.skipped = true;
        return Ok(report);
    }

    let results = futures::future::join_all(endpoint_ids.iter().map(|endpoint_id| {
        deliver_endpoint(
            database,
            repository,
            sender,
            cipher,
            *endpoint_id,
            worker_id,
            policy,
            env,
        )
    }))
    .await;
    let mut first_error = None;
    for result in results {
        match result {
            Ok(endpoint_report) => report.absorb(&endpoint_report),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(report),
    }
}

/// テスト送信 (永続化しない 1 回きりの送信) の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookTestOutcome {
    /// 環境変数の送信先が未設定。送信していない。
    NotConfigured,
    /// 環境変数の送信先の設定不備。送信していない。
    Misconfigured,
    Sent {
        event_id: String,
        delivered: bool,
        result: WebhookResultClass,
        http_status: Option<u16>,
        attempted_at: OffsetDateTime,
    },
}

/// 合成イベント (`webhook_test`) を実配送と同じ経路 (その送信先の署名・宛先検査・timeout) で 1 回送る。
/// 配送記録・通知イベント・ゲート状態・送信先の状態には一切書き込まない。
/// `destination` が `None` (secret を復号できない) なら送信せず `secret_undecryptable` を返す。
pub async fn send_webhook_test<S: WebhookSender>(
    sender: &S,
    destination: Option<&WebhookDestination>,
    now: OffsetDateTime,
) -> error_stack::Result<WebhookTestOutcome, KernelError> {
    let payload = WebhookPayload::test(crate::generate_id(), now);
    let body = payload.to_json()?;
    let observation = match destination {
        Some(destination) => {
            sender
                .send_to(
                    destination,
                    &WebhookRequest {
                        event_id: &payload.event_id,
                        event_type: WebhookEventType::WebhookTest,
                        attempt: 1,
                        body: body.as_bytes(),
                    },
                )
                .await
        }
        None => WebhookSendObservation::SecretUnavailable,
    };
    let (result, http_status) = crate::entity::classify_single_attempt(&observation);
    Ok(WebhookTestOutcome::Sent {
        event_id: payload.event_id,
        delivered: result == WebhookResultClass::Delivered,
        result,
        http_status,
        attempted_at: now,
    })
}
