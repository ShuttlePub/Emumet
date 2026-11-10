use application::service::registration_gate_webhook::WebhookAdminStatus;
use kernel::interfaces::repository::WebhookTestOutcome;
use kernel::prelude::entity::{
    WebhookConfigStatus, WebhookDelivery, WebhookDeliveryCounts, WebhookEventType,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

#[derive(Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct WebhookRetryRequest {
    /// 再試行する配送の event ID (通知イベント ID)。省略すると failed の配送すべてを再キューする。
    #[serde(default)]
    pub event_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookRetryResponse {
    /// 再キューした failed 配送の件数。同じ event ID が再送される。
    pub requeued: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookDeliveryCountsResponse {
    pub pending: i64,
    pub in_flight: i64,
    pub retrying: i64,
    pub delivered: i64,
    pub failed: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookDeliveryResponse {
    /// 通知イベント ID。Webhook の `X-Emumet-Event-Id` ヘッダーと同じ値 (再試行・再送で不変)。
    pub event_id: String,
    /// `outage_started` / `outage_recovered` / `protection_degraded` /
    /// `protection_lost_stopped` / `override_started` / `override_revoked`
    pub event_type: String,
    /// `pending` / `in_flight` / `retrying` / `delivered` / `failed`
    pub status: String,
    /// 通算の送信試行回数 (管理者の再試行をまたいで増える)。
    pub attempt_count: u32,
    /// 最後の試行結果の分類。secret・宛先 URL・応答本文・内部エラー詳細を含まない固定語彙
    /// (`delivered` / `timeout` / `connect_failed` / `dns_failed` / `http_request_timeout` /
    /// `http_too_many_requests` / `http_server_error` / `http_redirect` / `http_client_error` /
    /// `http_unexpected_status` / `ssrf_blocked` / `config_invalid` / `lease_expired`)。
    pub last_result: Option<String>,
    pub last_http_status: Option<u16>,
    /// failed のときの理由: `permanent_error` / `retries_exhausted`。
    pub failure_reason: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_attempt_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_attempt_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub delivered_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookStatusResponse {
    /// `not_configured` (未設定 = Webhook 無効。エラーではなく、配送もゲート動作への影響もない) /
    /// `misconfigured` (設定不備。`issue` を参照。ゲート動作には影響しない) / `configured`
    pub configuration: String,
    /// `misconfigured` のときの不備の分類 (`invalid_url` / `unsupported_scheme` /
    /// `credentials_in_url` / `missing_host` / `forbidden_destination` / `missing_secret` /
    /// `secret_too_short` / `secret_unreadable` / `ambiguous_secret_source` / `missing_url`)。
    pub issue: Option<String>,
    /// 宛先の host 部分のみ (userinfo・port・path・query・secret は出さない)。
    pub destination_host: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_success_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_failure_at: Option<OffsetDateTime>,
    pub counts: WebhookDeliveryCountsResponse,
    /// 新しい順の直近の配送記録 (本文は含まない)。
    pub recent_deliveries: Vec<WebhookDeliveryResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookTestResponse {
    /// テスト用の合成 event ID (`test_` で始まる。通知イベント ID とは衝突しない)。
    pub event_id: String,
    /// 受信側が 2xx を返したら true。
    pub delivered: bool,
    /// 結果の分類 (配送記録の `last_result` と同じ語彙)。
    pub result: String,
    pub http_status: Option<u16>,
    #[serde(with = "time::serde::rfc3339")]
    pub attempted_at: OffsetDateTime,
}

fn serde_str<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(value)) => value,
        _ => String::new(),
    }
}

impl From<&WebhookDeliveryCounts> for WebhookDeliveryCountsResponse {
    fn from(counts: &WebhookDeliveryCounts) -> Self {
        Self {
            pending: counts.pending,
            in_flight: counts.in_flight,
            retrying: counts.retrying,
            delivered: counts.delivered,
            failed: counts.failed,
        }
    }
}

impl From<&WebhookDelivery> for WebhookDeliveryResponse {
    fn from(delivery: &WebhookDelivery) -> Self {
        Self {
            event_id: delivery.event_id.as_ref().to_string(),
            event_type: serde_str(&WebhookEventType::from(delivery.kind)),
            status: serde_str(&delivery.status),
            attempt_count: delivery.attempt_count,
            last_result: delivery.last_result.as_ref().map(serde_str),
            last_http_status: delivery.last_http_status,
            failure_reason: delivery.failure_reason.as_ref().map(serde_str),
            next_attempt_at: delivery.next_attempt_at,
            last_attempt_at: delivery.last_attempt_at,
            delivered_at: delivery.delivered_at,
            created_at: delivery.created_at,
        }
    }
}

impl From<&WebhookAdminStatus> for WebhookStatusResponse {
    fn from(status: &WebhookAdminStatus) -> Self {
        let (configuration, issue, destination_host) = match &status.config {
            WebhookConfigStatus::Disabled => ("not_configured", None, None),
            WebhookConfigStatus::Misconfigured { issue } => {
                ("misconfigured", Some(serde_str(issue)), None)
            }
            WebhookConfigStatus::Enabled { host } => ("configured", None, Some(host.clone())),
        };
        Self {
            configuration: configuration.to_string(),
            issue,
            destination_host,
            last_success_at: status.summary.last_success_at,
            last_failure_at: status.summary.last_failure_at,
            counts: (&status.summary.counts).into(),
            recent_deliveries: status.recent.iter().map(Into::into).collect(),
        }
    }
}

impl WebhookTestResponse {
    /// 送信した場合のみ応答にできる (未設定・設定不備は呼び出し側が 409 にする)。
    pub fn from_outcome(outcome: &WebhookTestOutcome) -> Option<Self> {
        match outcome {
            WebhookTestOutcome::Sent {
                event_id,
                delivered,
                result,
                http_status,
                attempted_at,
            } => Some(Self {
                event_id: event_id.clone(),
                delivered: *delivered,
                result: serde_str(result),
                http_status: *http_status,
                attempted_at: *attempted_at,
            }),
            WebhookTestOutcome::NotConfigured | WebhookTestOutcome::Misconfigured => None,
        }
    }
}
