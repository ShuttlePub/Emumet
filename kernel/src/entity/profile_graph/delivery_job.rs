use crate::entity::{GraphActivityId, GraphInboxUrl, ProfileFollowId, ProfileId};
use crate::KernelError;
use error_stack::Report;
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use vodca::{AsRefln, Fromln, Newln};

const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct ProfileDeliveryJobId(i64);

/// 配送する Activity の種類 (D08 で Profile が送る関係 activity)。
///
/// `Block` は **個別 actor の block だけ**が送る (ユーザー決定。解除は元 Block を埋め込んだ `Undo`)。domain block は
/// `Block` を送らない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeliveryActivityType {
    Follow,
    Accept,
    Reject,
    /// 元の Follow または Block を埋め込む `Undo`。
    Undo,
    /// 個別 actor の block (`object` = 相手 actor)。
    Block,
}

impl DeliveryActivityType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Follow => "Follow",
            Self::Accept => "Accept",
            Self::Reject => "Reject",
            Self::Undo => "Undo",
            Self::Block => "Block",
        }
    }

    pub fn parse(value: &str) -> error_stack::Result<Self, KernelError> {
        match value {
            "Follow" => Ok(Self::Follow),
            "Accept" => Ok(Self::Accept),
            "Reject" => Ok(Self::Reject),
            "Undo" => Ok(Self::Undo),
            "Block" => Ok(Self::Block),
            other => Err(Report::new(KernelError::Internal)
                .attach_printable(format!("unknown delivery activity type: {other}"))),
        }
    }
}

/// 配送 job の状態。`Delivered` / `Failed` / `Cancelled` は終端。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileDeliveryJobState {
    /// 未試行。`next_attempt_at` 以降に取得できる。
    Pending,
    /// worker が lease を持って送信中。期限切れで他の worker が回収できる。
    InFlight,
    /// 一時的な失敗で `next_attempt_at` に再試行予定。
    Retrying,
    Delivered,
    Failed,
    Cancelled,
}

impl ProfileDeliveryJobState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InFlight => "in_flight",
            Self::Retrying => "retrying",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> error_stack::Result<Self, KernelError> {
        match value {
            "pending" => Ok(Self::Pending),
            "in_flight" => Ok(Self::InFlight),
            "retrying" => Ok(Self::Retrying),
            "delivered" => Ok(Self::Delivered),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(Report::new(KernelError::Internal)
                .attach_printable(format!("unknown delivery job state: {other}"))),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Delivered | Self::Failed | Self::Cancelled)
    }
}

/// 1 回の送信結果の分類 (固定語彙。応答本文・内部エラー詳細は保存しない)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryResultClass {
    Delivered,
    Timeout,
    ConnectFailed,
    DnsFailed,
    HttpRequestTimeout,
    HttpTooManyRequests,
    HttpServerError,
    HttpRedirect,
    HttpClientError,
    HttpUnexpectedStatus,
    /// 署名の生成に失敗した (送信していない)。
    SigningFailed,
    /// 宛先が不正または許可されない (送信していない)。
    InvalidTarget,
    /// 送信中に worker が停止し lease が期限切れになった (結果は不明)。
    LeaseExpired,
}

impl DeliveryResultClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Timeout => "timeout",
            Self::ConnectFailed => "connect_failed",
            Self::DnsFailed => "dns_failed",
            Self::HttpRequestTimeout => "http_request_timeout",
            Self::HttpTooManyRequests => "http_too_many_requests",
            Self::HttpServerError => "http_server_error",
            Self::HttpRedirect => "http_redirect",
            Self::HttpClientError => "http_client_error",
            Self::HttpUnexpectedStatus => "http_unexpected_status",
            Self::SigningFailed => "signing_failed",
            Self::InvalidTarget => "invalid_target",
            Self::LeaseExpired => "lease_expired",
        }
    }

    pub fn parse(value: &str) -> error_stack::Result<Self, KernelError> {
        Ok(match value {
            "delivered" => Self::Delivered,
            "timeout" => Self::Timeout,
            "connect_failed" => Self::ConnectFailed,
            "dns_failed" => Self::DnsFailed,
            "http_request_timeout" => Self::HttpRequestTimeout,
            "http_too_many_requests" => Self::HttpTooManyRequests,
            "http_server_error" => Self::HttpServerError,
            "http_redirect" => Self::HttpRedirect,
            "http_client_error" => Self::HttpClientError,
            "http_unexpected_status" => Self::HttpUnexpectedStatus,
            "signing_failed" => Self::SigningFailed,
            "invalid_target" => Self::InvalidTarget,
            "lease_expired" => Self::LeaseExpired,
            other => {
                return Err(Report::new(KernelError::Internal)
                    .attach_printable(format!("unknown delivery result class: {other}")))
            }
        })
    }
}

/// `Failed` になった理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryFailureReason {
    /// 再試行しても結果が変わらない失敗。
    PermanentError,
    /// 一時的な失敗が worker の定める上限を超えた (lease 期限切れの繰り返しを含む)。
    RetriesExhausted,
}

impl DeliveryFailureReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PermanentError => "permanent_error",
            Self::RetriesExhausted => "retries_exhausted",
        }
    }

    pub fn parse(value: &str) -> error_stack::Result<Self, KernelError> {
        match value {
            "permanent_error" => Ok(Self::PermanentError),
            "retries_exhausted" => Ok(Self::RetriesExhausted),
            other => Err(Report::new(KernelError::Internal)
                .attach_printable(format!("unknown delivery failure reason: {other}"))),
        }
    }
}

/// enqueue する配送 job (P3o / P3i が共通に組み立てる)。
///
/// - `(sender, activity_id, target_inbox)` が冪等性キー。同じ組の再 enqueue は新しい job を作らない。
/// - `payload` は **送信する本文のバイト列そのもの** (JSON オブジェクト)。HTTP 署名の digest は本文の
///   バイト列に依存するので、DB には正規化されない TEXT として保存する。
/// - `follow_id` は関係との紐付け (取消・追跡用)。任意。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProfileDeliveryJob {
    pub id: ProfileDeliveryJobId,
    pub sender: ProfileId,
    pub activity_type: DeliveryActivityType,
    pub activity_id: GraphActivityId,
    pub target_inbox: GraphInboxUrl,
    pub payload: String,
    pub follow_id: Option<ProfileFollowId>,
    /// 最初の試行を許可する時刻。
    pub not_before: OffsetDateTime,
    pub created_at: OffsetDateTime,
}

impl NewProfileDeliveryJob {
    /// 即時に試行できる job を作る。ID は snowflake で採番する。
    pub fn new(
        sender: ProfileId,
        activity_type: DeliveryActivityType,
        activity_id: GraphActivityId,
        target_inbox: GraphInboxUrl,
        payload: impl Into<String>,
        now: OffsetDateTime,
    ) -> error_stack::Result<Self, KernelError> {
        let payload = payload.into();
        if payload.len() > MAX_PAYLOAD_BYTES {
            return Err(Report::new(KernelError::Validation)
                .attach_printable("delivery payload is too large"));
        }
        match serde_json::from_str::<serde_json::Value>(&payload) {
            Ok(serde_json::Value::Object(_)) => {}
            _ => {
                return Err(Report::new(KernelError::Validation)
                    .attach_printable("delivery payload must be a JSON object"))
            }
        }
        Ok(Self {
            id: ProfileDeliveryJobId::new(crate::generate_id()),
            sender,
            activity_type,
            activity_id,
            target_inbox,
            payload,
            follow_id: None,
            not_before: now,
            created_at: now,
        })
    }

    pub fn with_follow(mut self, follow_id: ProfileFollowId) -> Self {
        self.follow_id = Some(follow_id);
        self
    }

    pub fn not_before(mut self, not_before: OffsetDateTime) -> Self {
        self.not_before = not_before;
        self
    }
}

/// `ProfileDeliveryEnqueuer::enqueue` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Enqueued(ProfileDeliveryJobId),
    /// 同じ冪等性キーの job が既にある (状態は問わない)。何も変更していない。
    AlreadyEnqueued(ProfileDeliveryJobId),
}

impl EnqueueOutcome {
    pub fn job_id(&self) -> &ProfileDeliveryJobId {
        match self {
            Self::Enqueued(id) | Self::AlreadyEnqueued(id) => id,
        }
    }

    pub fn is_newly_enqueued(&self) -> bool {
        matches!(self, Self::Enqueued(_))
    }
}

/// 保存済みの配送 job。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileDeliveryJob {
    pub id: ProfileDeliveryJobId,
    pub sender: ProfileId,
    pub activity_type: DeliveryActivityType,
    pub activity_id: GraphActivityId,
    pub target_inbox: GraphInboxUrl,
    pub payload: String,
    pub follow_id: Option<ProfileFollowId>,
    pub state: ProfileDeliveryJobState,
    /// 取得された回数 (lease を取るたびに 1 増える)。
    pub attempt_count: i32,
    pub next_attempt_at: OffsetDateTime,
    pub last_attempt_at: Option<OffsetDateTime>,
    pub last_result: Option<DeliveryResultClass>,
    pub last_http_status: Option<i32>,
    pub failure_reason: Option<DeliveryFailureReason>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<OffsetDateTime>,
    pub delivered_at: Option<OffsetDateTime>,
    pub cancelled_at: Option<OffsetDateTime>,
    pub cancel_reason: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// 試行の履歴 1 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileDeliveryAttempt {
    pub job_id: ProfileDeliveryJobId,
    pub attempt_no: i32,
    pub started_at: OffsetDateTime,
    pub finished_at: OffsetDateTime,
    pub result: DeliveryResultClass,
    pub http_status: Option<i32>,
}

/// `claim_due` の設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryClaimPolicy {
    pub lease_duration: Duration,
    /// 1 回で取得する最大件数。
    pub limit: i64,
    /// lease 期限切れの回収でこの回数に達した job は再取得せず `Failed(retries_exhausted)` にする。
    pub max_attempts: i32,
}

/// 取得した job。`lease_token` を `record_attempt` に渡して結果を記録する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedProfileDeliveryJob {
    pub id: ProfileDeliveryJobId,
    pub sender: ProfileId,
    pub activity_type: DeliveryActivityType,
    pub activity_id: GraphActivityId,
    pub target_inbox: GraphInboxUrl,
    pub payload: String,
    pub follow_id: Option<ProfileFollowId>,
    /// 今回の試行番号 (1 始まり)。
    pub attempt_no: i32,
    pub started_at: OffsetDateTime,
    pub lease_owner: String,
    /// 今回の lease を識別するトークン。期限切れ後に別 worker が再取得すると変わるので、
    /// 古い worker の結果記録は拒否される。
    pub lease_token: String,
    pub lease_expires_at: OffsetDateTime,
}

/// 送信結果。次の状態の決定 (バックオフ・上限判断) は worker (P3o) が行い、保存層は結果を記録するだけ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryAttemptOutcome {
    Delivered {
        http_status: Option<i32>,
    },
    Retry {
        next_attempt_at: OffsetDateTime,
        result: DeliveryResultClass,
        http_status: Option<i32>,
    },
    Failed {
        reason: DeliveryFailureReason,
        result: DeliveryResultClass,
        http_status: Option<i32>,
    },
}

impl DeliveryAttemptOutcome {
    pub fn result_class(&self) -> DeliveryResultClass {
        match self {
            Self::Delivered { .. } => DeliveryResultClass::Delivered,
            Self::Retry { result, .. } | Self::Failed { result, .. } => *result,
        }
    }

    pub fn http_status(&self) -> Option<i32> {
        match self {
            Self::Delivered { http_status }
            | Self::Retry { http_status, .. }
            | Self::Failed { http_status, .. } => *http_status,
        }
    }
}

/// `record_attempt` の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordAttemptOutcome {
    /// 結果を記録し、job の状態を更新した。
    Recorded,
    /// 送信中に job が取り消された。試行の履歴だけ残し、状態は `Cancelled` のまま。
    /// 取消は送信済みの activity を取り戻さない。
    Cancelled,
    /// lease を失っていた (期限切れで他 worker が回収した等)。何も記録しない。
    LeaseLost,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(payload: &str) -> error_stack::Result<NewProfileDeliveryJob, KernelError> {
        crate::ensure_generator_initialized();
        NewProfileDeliveryJob::new(
            ProfileId::new(1),
            DeliveryActivityType::Follow,
            GraphActivityId::new("https://local.example/activities/1").unwrap(),
            GraphInboxUrl::new("https://remote.example/inbox").unwrap(),
            payload,
            OffsetDateTime::now_utc(),
        )
    }

    #[test]
    fn payload_must_be_json_object() {
        assert!(make(r#"{"type":"Follow"}"#).is_ok());
        assert!(make("[]").is_err());
        assert!(make("\"x\"").is_err());
        assert!(make("not json").is_err());
    }

    #[test]
    fn payload_keeps_exact_bytes() {
        let raw = "{\"b\": 1,   \"a\":2}";
        assert_eq!(make(raw).unwrap().payload, raw);
    }

    #[test]
    fn payload_size_is_bounded() {
        let big = format!("{{\"x\":\"{}\"}}", "a".repeat(MAX_PAYLOAD_BYTES));
        assert!(make(&big).is_err());
    }

    #[test]
    fn enum_text_round_trip() {
        for t in [
            DeliveryActivityType::Follow,
            DeliveryActivityType::Accept,
            DeliveryActivityType::Reject,
            DeliveryActivityType::Undo,
        ] {
            assert_eq!(DeliveryActivityType::parse(t.as_str()).unwrap(), t);
        }
        for s in [
            ProfileDeliveryJobState::Pending,
            ProfileDeliveryJobState::InFlight,
            ProfileDeliveryJobState::Retrying,
            ProfileDeliveryJobState::Delivered,
            ProfileDeliveryJobState::Failed,
            ProfileDeliveryJobState::Cancelled,
        ] {
            assert_eq!(ProfileDeliveryJobState::parse(s.as_str()).unwrap(), s);
        }
        for r in [
            DeliveryResultClass::Delivered,
            DeliveryResultClass::Timeout,
            DeliveryResultClass::ConnectFailed,
            DeliveryResultClass::DnsFailed,
            DeliveryResultClass::HttpRequestTimeout,
            DeliveryResultClass::HttpTooManyRequests,
            DeliveryResultClass::HttpServerError,
            DeliveryResultClass::HttpRedirect,
            DeliveryResultClass::HttpClientError,
            DeliveryResultClass::HttpUnexpectedStatus,
            DeliveryResultClass::SigningFailed,
            DeliveryResultClass::InvalidTarget,
            DeliveryResultClass::LeaseExpired,
        ] {
            assert_eq!(DeliveryResultClass::parse(r.as_str()).unwrap(), r);
        }
        assert!(DeliveryResultClass::parse("x").is_err());
    }
}
