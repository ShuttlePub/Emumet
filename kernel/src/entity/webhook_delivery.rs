//! 登録ゲート通知の外部 Webhook 配送 (D15 / E15w) のドメイン。
//!
//! E15a が永続化した通知イベントを、サーバー設定の単一 HTTPS endpoint へ署名付き・at-least-once で
//! 配送する。ここには I/O のない **純粋ロジック** (結果分類・バックオフ・payload allowlist) と、
//! 配送記録の型だけを置く。
//!
//! 重要な不変条件:
//! - 配送対象は `RegistrationGateEvent` (E15a の通知イベント) だけ。配送の失敗・failed・設定不備から
//!   新しい通知イベントを作る経路は存在しない (`NewWebhookDelivery` は `RegistrationGateEvent` からのみ
//!   構築でき、配送記録は通知イベントを生成しない)。
//! - payload は allowlist 方式の型でしか組み立てない。登録者のメールアドレス・secret・内部エラー詳細・
//!   操作した管理者の ID を含む経路を持たない。

use crate::entity::{
    AdminState, GateControlSnapshot, GateEventKind, HealthEpisode, ProbeStatus,
    ProtectionObservation, RegistrationGateEvent, RegistrationGateEventId,
};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

/// 配送記録の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookDeliveryStatus {
    /// 未送信 (初回、または管理者の再試行で再キューされた)。
    Pending,
    /// worker が lease を取って送信中。lease 期限切れで回収される。
    InFlight,
    /// 一時的な失敗で `next_attempt_at` に再試行予定。
    Retrying,
    Delivered,
    /// 恒久失敗または再試行上限超過。管理者が再試行を指示するまで再送しない。
    Failed,
}

/// 1 回の送信結果の分類。secret・宛先 URL・応答本文・内部エラー詳細を含まない固定語彙。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookResultClass {
    Delivered,
    /// 全体 timeout。
    Timeout,
    /// 接続失敗 (TLS 失敗を含む)。
    ConnectFailed,
    /// 名前解決の失敗・空応答 (一時的なものとして扱う)。
    DnsFailed,
    /// HTTP 408。
    HttpRequestTimeout,
    /// HTTP 429。
    HttpTooManyRequests,
    /// HTTP 5xx。
    HttpServerError,
    /// HTTP 3xx (リダイレクトは追従しない)。
    HttpRedirect,
    /// 408/429 以外の 4xx。
    HttpClientError,
    /// 1xx など想定外の status。
    HttpUnexpectedStatus,
    /// 宛先検査で拒否 (内部アドレスへ解決された等)。
    SsrfBlocked,
    /// 設定不備 (未設定・不正な URL/secret)。
    ConfigInvalid,
    /// 送信中に worker が停止し lease が期限切れになった (結果は不明)。
    LeaseExpired,
}

/// failed になった理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookFailureReason {
    /// 再試行しても結果が変わらない失敗 (3xx・その他 4xx・SSRF 拒否・設定不備)。
    PermanentError,
    /// 一時的な失敗が上限回数または最大期間を超えた。
    RetriesExhausted,
}

/// 配送の再試行方針。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookDeliveryPolicy {
    /// 1 サイクル (初回投入、または管理者の再試行 1 回) あたりの最大試行回数。
    pub max_attempts: u32,
    /// バックオフの基準 (1 回目の失敗後)。以降 2 倍ずつ。
    pub base_delay: Duration,
    pub max_delay: Duration,
    /// `Retry-After` を尊重する上限。
    pub retry_after_cap: Duration,
    /// サイクル開始からこの期間を超えて再試行しない。
    pub max_age: Duration,
    /// 送信中 lease の長さ。lease は送信の直前に 1 件ずつ取るので、sender の名前解決 + 全体 timeout
    /// より長ければ送信中に切れない。
    pub lease_duration: Duration,
    /// 1 回の配送サイクルで処理する最大件数 (lease は 1 件ずつ取る)。
    pub batch_size: i64,
}

impl Default for WebhookDeliveryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 8,
            base_delay: Duration::seconds(30),
            max_delay: Duration::hours(1),
            retry_after_cap: Duration::hours(1),
            max_age: Duration::hours(24),
            lease_duration: Duration::seconds(120),
            batch_size: 10,
        }
    }
}

/// sender が観測した 1 回の送信結果 (分類前)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookSendObservation {
    /// 応答を受信した。`retry_after` は 429/503 の `Retry-After` を解釈した値。
    Response {
        status: u16,
        retry_after: Option<Duration>,
    },
    Timeout,
    ConnectFailed,
    DnsFailed,
    /// 解決後アドレスの検査で拒否された。
    SsrfBlocked,
    /// 送信できる設定がない・不正。
    ConfigInvalid,
}

/// 1 回の送信結果に対する扱い。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookDecision {
    Delivered {
        http_status: Option<u16>,
    },
    Retry {
        class: WebhookResultClass,
        http_status: Option<u16>,
        delay: Duration,
    },
    Failed {
        class: WebhookResultClass,
        http_status: Option<u16>,
        reason: WebhookFailureReason,
    },
}

impl WebhookDecision {
    pub fn class(&self) -> WebhookResultClass {
        match self {
            WebhookDecision::Delivered { .. } => WebhookResultClass::Delivered,
            WebhookDecision::Retry { class, .. } | WebhookDecision::Failed { class, .. } => *class,
        }
    }

    pub fn http_status(&self) -> Option<u16> {
        match self {
            WebhookDecision::Delivered { http_status }
            | WebhookDecision::Retry { http_status, .. }
            | WebhookDecision::Failed { http_status, .. } => *http_status,
        }
    }
}

/// 観測が一時的な失敗か恒久失敗か成功かの分類 (再試行上限の判定前)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Classification {
    Success,
    Transient,
    Permanent,
}

fn classify(observation: &WebhookSendObservation) -> (Classification, WebhookResultClass) {
    use Classification::*;
    use WebhookResultClass as C;
    match observation {
        WebhookSendObservation::Response { status, .. } => match *status {
            200..=299 => (Success, C::Delivered),
            300..=399 => (Permanent, C::HttpRedirect),
            408 => (Transient, C::HttpRequestTimeout),
            429 => (Transient, C::HttpTooManyRequests),
            500..=599 => (Transient, C::HttpServerError),
            400..=499 => (Permanent, C::HttpClientError),
            _ => (Permanent, C::HttpUnexpectedStatus),
        },
        WebhookSendObservation::Timeout => (Transient, C::Timeout),
        WebhookSendObservation::ConnectFailed => (Transient, C::ConnectFailed),
        WebhookSendObservation::DnsFailed => (Transient, C::DnsFailed),
        WebhookSendObservation::SsrfBlocked => (Permanent, C::SsrfBlocked),
        WebhookSendObservation::ConfigInvalid => (Permanent, C::ConfigInvalid),
    }
}

fn observed_status(observation: &WebhookSendObservation) -> Option<u16> {
    match observation {
        WebhookSendObservation::Response { status, .. } => Some(*status),
        _ => None,
    }
}

/// `cycle_attempt` (1 始まり。今回の試行を含む) 回目の失敗後のバックオフ。
/// `base_delay * 2^(n-1)` を `max_delay` で頭打ちにし、`[50%, 100%]` の範囲で jitter をかける。
/// `jitter` は `[0, 1)` の一様乱数 (範囲外は丸める)。
pub fn backoff_delay(cycle_attempt: u32, policy: &WebhookDeliveryPolicy, jitter: f64) -> Duration {
    let exponent = cycle_attempt.saturating_sub(1).min(30);
    let base_ms = policy
        .base_delay
        .whole_milliseconds()
        .clamp(0, i64::MAX as i128) as i64;
    let max_ms = policy
        .max_delay
        .whole_milliseconds()
        .clamp(0, i64::MAX as i128) as i64;
    let raw_ms = base_ms.saturating_mul(1i64 << exponent).min(max_ms);
    let jitter = if jitter.is_finite() {
        jitter.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let jittered_ms = (raw_ms as f64 * (0.5 + 0.5 * jitter)) as i64;
    Duration::milliseconds(jittered_ms)
}

/// 送信結果の扱いを決める。
///
/// - 2xx: Delivered
/// - 408 / 429 / 5xx / timeout / 接続失敗 / DNS 失敗: 再試行 (指数バックオフ + jitter。429 と 503 の
///   `Retry-After` は `retry_after_cap` を上限に尊重)。回数上限・最大期間を超えたら failed
/// - 3xx・その他の 4xx・SSRF 拒否・設定不備: 恒久失敗 (failed)
///
/// `cycle_attempt` は今回の試行を含む 1 始まりの回数、`cycle_age` はサイクル開始からの経過時間。
pub fn decide_delivery(
    observation: &WebhookSendObservation,
    cycle_attempt: u32,
    cycle_age: Duration,
    policy: &WebhookDeliveryPolicy,
    jitter: f64,
) -> WebhookDecision {
    let (classification, class) = classify(observation);
    let http_status = observed_status(observation);
    match classification {
        Classification::Success => WebhookDecision::Delivered { http_status },
        Classification::Permanent => WebhookDecision::Failed {
            class,
            http_status,
            reason: WebhookFailureReason::PermanentError,
        },
        Classification::Transient => {
            let mut delay = backoff_delay(cycle_attempt, policy, jitter);
            if let WebhookSendObservation::Response {
                status: 429 | 503,
                retry_after: Some(retry_after),
            } = observation
            {
                let honored = (*retry_after).clamp(Duration::ZERO, policy.retry_after_cap);
                delay = delay.max(honored);
            }
            if cycle_attempt >= policy.max_attempts || cycle_age + delay > policy.max_age {
                WebhookDecision::Failed {
                    class,
                    http_status,
                    reason: WebhookFailureReason::RetriesExhausted,
                }
            } else {
                WebhookDecision::Retry {
                    class,
                    http_status,
                    delay,
                }
            }
        }
    }
}

/// テスト送信 (永続化しない 1 回きりの送信) の結果の分類。
pub fn classify_single_attempt(
    observation: &WebhookSendObservation,
) -> (WebhookResultClass, Option<u16>) {
    let (_, class) = classify(observation);
    (class, observed_status(observation))
}

// ---------------------------------------------------------------------------------------------
// 設定状態
// ---------------------------------------------------------------------------------------------

/// 設定不備の分類。secret・URL の値そのものを含まない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookConfigIssue {
    /// URL として解釈できない (IPv6 zone 付きなど)。
    InvalidUrl,
    /// https 以外のスキーム。
    UnsupportedScheme,
    /// URL に認証情報 (userinfo) が含まれる。
    CredentialsInUrl,
    MissingHost,
    /// localhost / 内部アドレスなど、検査で恒久拒否される固定宛先。
    ForbiddenDestination,
    /// URL はあるが署名 secret が未設定。
    MissingSecret,
    /// secret が短すぎる。
    SecretTooShort,
    /// secret ファイルを読めない・空。
    SecretUnreadable,
    /// secret の直接指定とファイル指定が同時にある。
    AmbiguousSecretSource,
    /// secret だけがあり URL が未設定。
    MissingUrl,
}

/// 管理者 API に出す設定状態。宛先は host 部分のみ (userinfo・path・query は出さない)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WebhookConfigStatus {
    /// 未設定 = Webhook 無効 (エラーではない)。
    Disabled,
    Misconfigured {
        issue: WebhookConfigIssue,
    },
    Enabled {
        host: String,
    },
}

impl WebhookConfigStatus {
    pub fn is_enabled(&self) -> bool {
        matches!(self, WebhookConfigStatus::Enabled { .. })
    }
}

// ---------------------------------------------------------------------------------------------
// payload (allowlist)
// ---------------------------------------------------------------------------------------------

/// Webhook で配送するイベント種別。E15a の通知イベント全種別 + 管理者のテスト送信。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEventType {
    OutageStarted,
    OutageRecovered,
    ProtectionDegraded,
    ProtectionLostStopped,
    EnforcementLostStopped,
    EnforcementRestored,
    OverrideStarted,
    OverrideRevoked,
    /// 管理者のテスト送信 (合成イベント。通知一覧・ゲート状態には現れない)。
    WebhookTest,
}

impl WebhookEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            WebhookEventType::OutageStarted => "outage_started",
            WebhookEventType::OutageRecovered => "outage_recovered",
            WebhookEventType::ProtectionDegraded => "protection_degraded",
            WebhookEventType::ProtectionLostStopped => "protection_lost_stopped",
            WebhookEventType::EnforcementLostStopped => "enforcement_lost_stopped",
            WebhookEventType::EnforcementRestored => "enforcement_restored",
            WebhookEventType::OverrideStarted => "override_started",
            WebhookEventType::OverrideRevoked => "override_revoked",
            WebhookEventType::WebhookTest => "webhook_test",
        }
    }
}

impl From<GateEventKind> for WebhookEventType {
    fn from(kind: GateEventKind) -> Self {
        match kind {
            GateEventKind::OutageStarted => WebhookEventType::OutageStarted,
            GateEventKind::OutageRecovered => WebhookEventType::OutageRecovered,
            GateEventKind::ProtectionDegraded => WebhookEventType::ProtectionDegraded,
            GateEventKind::ProtectionLostStopped => WebhookEventType::ProtectionLostStopped,
            GateEventKind::EnforcementLostStopped => WebhookEventType::EnforcementLostStopped,
            GateEventKind::EnforcementRestored => WebhookEventType::EnforcementRestored,
            GateEventKind::OverrideStarted => WebhookEventType::OverrideStarted,
            GateEventKind::OverrideRevoked => WebhookEventType::OverrideRevoked,
        }
    }
}

/// payload の schema version。互換性を壊す変更のときだけ上げる。
pub const WEBHOOK_PAYLOAD_VERSION: u8 = 1;

/// ゲート状態の要約 (そのイベントを生んだ遷移の直後の制御状態)。
fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookGateSummary {
    pub admin_state: AdminState,
    pub override_active: bool,
    pub config_lost_latch: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub enforcement_lost_latch: bool,
    pub episode: HealthEpisode,
    pub revision: i64,
}

impl From<&GateControlSnapshot> for WebhookGateSummary {
    fn from(snapshot: &GateControlSnapshot) -> Self {
        Self {
            admin_state: snapshot.admin,
            override_active: snapshot.override_active,
            config_lost_latch: snapshot.config_lost_latch,
            enforcement_lost_latch: snapshot.enforcement_lost_latch,
            episode: snapshot.episode,
            revision: snapshot.revision,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookProtectionSummary {
    pub configured: bool,
    pub probe: ProbeStatus,
}

impl From<&ProtectionObservation> for WebhookProtectionSummary {
    fn from(observation: &ProtectionObservation) -> Self {
        Self {
            configured: observation.configured,
            probe: observation.probe,
        }
    }
}

/// Webhook の JSON 本文。**allowlist**: ここに列挙したフィールドだけが本文に出る。
/// 登録者のメールアドレス・secret・操作した管理者・内部エラー詳細は型として存在しない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookPayload {
    pub version: u8,
    /// 安定した event ID (再試行・再送・再起動で同一)。受信側はこれで重複排除する。
    pub event_id: String,
    #[serde(rename = "type")]
    pub event_type: WebhookEventType,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<WebhookGateSummary>,
    /// メール認証 (設定上の有無と検証 API の観測)。登録者のメールアドレスではない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_verification: Option<WebhookProtectionSummary>,
    /// BOT 対策 (設定上の有無と検証 API の観測)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_protection: Option<WebhookProtectionSummary>,
}

/// E15a の通知イベントの Webhook 上の event ID (10 進文字列。通知一覧 API の event ID と同じ)。
pub fn gate_event_wire_id(id: &RegistrationGateEventId) -> String {
    id.as_ref().to_string()
}

/// テスト送信の event ID。通知イベントの ID 空間と衝突しないよう接頭辞を付ける。
pub fn test_event_wire_id(id: i64) -> String {
    format!("test_{id}")
}

impl WebhookPayload {
    /// 通知イベントの payload。`gate` はそのイベントを生んだ遷移直後の制御状態。
    pub fn from_gate_event(event: &RegistrationGateEvent, gate: &GateControlSnapshot) -> Self {
        Self {
            version: WEBHOOK_PAYLOAD_VERSION,
            event_id: gate_event_wire_id(&event.id),
            event_type: event.kind.into(),
            occurred_at: event.created_at,
            gate: Some(gate.into()),
            email_verification: Some((&event.observation.email).into()),
            bot_protection: Some((&event.observation.bot).into()),
        }
    }

    /// 管理者のテスト送信用の合成 payload。ゲート状態の要約は含めない。
    pub fn test(id: i64, now: OffsetDateTime) -> Self {
        Self {
            version: WEBHOOK_PAYLOAD_VERSION,
            event_id: test_event_wire_id(id),
            event_type: WebhookEventType::WebhookTest,
            occurred_at: now,
            gate: None,
            email_verification: None,
            bot_protection: None,
        }
    }

    pub fn to_json(&self) -> error_stack::Result<String, crate::KernelError> {
        serde_json::to_string(self).map_err(|error| {
            error_stack::Report::new(error).change_context(crate::KernelError::Internal)
        })
    }
}

// ---------------------------------------------------------------------------------------------
// 配送記録
// ---------------------------------------------------------------------------------------------

/// 取り込む配送記録。通知イベント (E15a) からのみ作れる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWebhookDelivery {
    pub event_id: RegistrationGateEventId,
    pub kind: GateEventKind,
    /// 配送する JSON 本文 (取り込み時に確定し、再試行・再送でも同一のバイト列を送る)。
    pub payload_json: String,
}

impl NewWebhookDelivery {
    pub fn from_gate_event(
        event: &RegistrationGateEvent,
        gate: &GateControlSnapshot,
    ) -> error_stack::Result<Self, crate::KernelError> {
        Ok(Self {
            event_id: event.id.clone(),
            kind: event.kind,
            payload_json: WebhookPayload::from_gate_event(event, gate).to_json()?,
        })
    }
}

/// worker が lease を取って送信する配送。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedWebhookDelivery {
    pub event_id: RegistrationGateEventId,
    pub kind: GateEventKind,
    pub payload_json: String,
    /// この配送記録の通算試行番号 (1 始まり。管理者の再試行をまたいで増える)。
    pub attempt: u32,
    /// 現在のサイクル内の試行回数 (今回を含む 1 始まり)。上限判定に使う。
    pub cycle_attempt: u32,
    pub cycle_started_at: OffsetDateTime,
    pub claimed_at: OffsetDateTime,
}

/// 管理者 API に出す配送記録 (本文は含めない)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookDelivery {
    pub event_id: RegistrationGateEventId,
    pub kind: GateEventKind,
    pub status: WebhookDeliveryStatus,
    pub attempt_count: u32,
    pub next_attempt_at: Option<OffsetDateTime>,
    pub last_attempt_at: Option<OffsetDateTime>,
    pub last_result: Option<WebhookResultClass>,
    pub last_http_status: Option<u16>,
    pub failure_reason: Option<WebhookFailureReason>,
    pub lease_expires_at: Option<OffsetDateTime>,
    pub delivered_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

/// 試行履歴 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookAttempt {
    pub attempt: u32,
    pub started_at: OffsetDateTime,
    pub finished_at: OffsetDateTime,
    pub result: WebhookResultClass,
    pub http_status: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebhookDeliveryCounts {
    pub pending: i64,
    pub in_flight: i64,
    pub retrying: i64,
    pub delivered: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebhookDeliverySummary {
    pub counts: WebhookDeliveryCounts,
    pub last_success_at: Option<OffsetDateTime>,
    pub last_failure_at: Option<OffsetDateTime>,
}

#[cfg(test)]
mod tests;
