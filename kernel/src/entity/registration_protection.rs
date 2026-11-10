//! 登録保護の readiness (D15 / E15c) のドメイン。
//!
//! 保護 (メール所有確認 / BOT 対策) ごとに「実際の検証 API の business 結果」を **設定世代 (generation) に
//! 束縛** して記録し、鮮度期限付きで評価する **純粋ロジック** だけを置く (I/O なし・時刻は引数)。
//!
//! 不変条件:
//! - `operational` になる根拠は business success だけ。HTTP 200・SMTP 250・接続成功・health は
//!   `Accepted` (= 資格情報/接続が受理された) までで、operational の根拠にならない。
//! - 古い成功を無期限に使わない: generation 不一致・成功期限切れ・probe 停止は `unverified`。
//! - `attempt_rejected` (当該登録者の不正入力) は readiness も観測入力も変えない。
//! - 設定喪失 (`configuration_invalid`・未設定・明示無効) と一時障害を区別する。
//! - reason / transport は secret・token・メールアドレスを含まない固定語彙。

use crate::entity::{ProbeStatus, Protection, ProtectionObservation};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

/// 検証 provider の種類 (保護ごとに固定)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectionProvider {
    Smtp,
    Turnstile,
}

impl ProtectionProvider {
    pub fn for_protection(protection: Protection) -> Self {
        match protection {
            Protection::Email => Self::Smtp,
            Protection::Bot => Self::Turnstile,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Smtp => "smtp",
            Self::Turnstile => "turnstile",
        }
    }
}

/// 設定世代。保護ごとの有効設定 (provider・enabled・非 secret の設定値・secret の逆算できない
/// fingerprint) から決定的に導かれ、設定が変われば変わる。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfigGeneration(String);

impl ConfigGeneration {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ConfigGeneration {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// readiness の状態 (ADR の分類表)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStatus {
    /// 未設定・空設定・明示無効・無効資格情報・SMTP 認証/設定不正。再設定が必要。
    ConfigurationInvalid,
    /// business success が当該 generation に束縛され、鮮度内。
    Operational,
    /// 429/5xx/timeout/接続失敗/不正応答/SMTP 4xx。設定は失っていない。
    TemporarilyUnavailable,
    /// 設定変更後・成功期限切れ・probe 停止・未検証。
    Unverified,
}

impl ReadinessStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ConfigurationInvalid => "configuration_invalid",
            Self::Operational => "operational",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::Unverified => "unverified",
        }
    }

    /// E15a の観測入力への固定対応 (契約 §4)。
    ///
    /// - operational → configured=true・Healthy
    /// - temporarily_unavailable → configured=true・Unhealthy
    /// - unverified → configured=true・Unknown
    /// - configuration_invalid → configured=false
    pub fn to_observation(self) -> ProtectionObservation {
        match self {
            Self::Operational => ProtectionObservation::new(true, ProbeStatus::Healthy),
            Self::TemporarilyUnavailable => {
                ProtectionObservation::new(true, ProbeStatus::Unhealthy)
            }
            Self::Unverified => ProtectionObservation::new(true, ProbeStatus::Unknown),
            Self::ConfigurationInvalid => ProtectionObservation::UNCONFIGURED,
        }
    }
}

/// 状態・検証結果の理由。secret・token・メールアドレスを含まない固定語彙。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    // --- configuration_invalid ---
    /// 設定が一切ない / 空。
    NotConfigured,
    /// 明示的に無効化されている。
    Disabled,
    /// 設定値の不備 (不正な host・port・from address・hostname 等)。
    InvalidConfiguration,
    /// secret が未設定・空。
    MissingSecret,
    /// provider が secret を拒否した (`invalid-input-secret` 等)。
    InvalidSecret,
    /// SMTP 認証失敗 (530/534/535/538)。
    SmtpAuthFailed,
    /// SMTP の恒久的な設定不正 (認証以外の 5xx)。
    SmtpPermanentFailure,
    /// SMTP の TLS 設定不正 (証明書・handshake)。
    SmtpTlsFailure,
    // --- temporarily_unavailable ---
    ProviderRateLimited,
    ProviderServerError,
    Timeout,
    ConnectFailed,
    TlsFailed,
    MalformedResponse,
    SmtpTransientFailure,
    // --- attempt_rejected (当該試行のみ) ---
    TokenRejected,
    TokenDuplicate,
    TokenExpired,
    HostnameMismatch,
    ActionMismatch,
    RecipientRejected,
    /// 宛先を伴う RCPT 段階の一時拒否 (greylisting・mailbox busy など宛先依存)。provider の障害ではない。
    RecipientDeferred,
    // --- 肯定的な結果 ---
    BusinessSuccess,
    /// 資格情報・接続が受理された (business success ではない)。
    Accepted,
    // --- unverified ---
    NeverVerified,
    GenerationChanged,
    ProbeStale,
    SuccessExpired,
    NoBusinessSuccess,
}

impl ReasonCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::Disabled => "disabled",
            Self::InvalidConfiguration => "invalid_configuration",
            Self::MissingSecret => "missing_secret",
            Self::InvalidSecret => "invalid_secret",
            Self::SmtpAuthFailed => "smtp_auth_failed",
            Self::SmtpPermanentFailure => "smtp_permanent_failure",
            Self::SmtpTlsFailure => "smtp_tls_failure",
            Self::ProviderRateLimited => "provider_rate_limited",
            Self::ProviderServerError => "provider_server_error",
            Self::Timeout => "timeout",
            Self::ConnectFailed => "connect_failed",
            Self::TlsFailed => "tls_failed",
            Self::MalformedResponse => "malformed_response",
            Self::SmtpTransientFailure => "smtp_transient_failure",
            Self::TokenRejected => "token_rejected",
            Self::TokenDuplicate => "token_duplicate",
            Self::TokenExpired => "token_expired",
            Self::HostnameMismatch => "hostname_mismatch",
            Self::ActionMismatch => "action_mismatch",
            Self::RecipientRejected => "recipient_rejected",
            Self::RecipientDeferred => "recipient_deferred",
            Self::BusinessSuccess => "business_success",
            Self::Accepted => "accepted",
            Self::NeverVerified => "never_verified",
            Self::GenerationChanged => "generation_changed",
            Self::ProbeStale => "probe_stale",
            Self::SuccessExpired => "success_expired",
            Self::NoBusinessSuccess => "no_business_success",
        }
    }
}

/// 検証 API の transport 層の分類 (business 結果とは別に記録する。HTTP 200 を成功と同一視しない)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportClass {
    /// 通信していない (設定不備・fixture 等)。
    None,
    Http2xx,
    Http4xx,
    Http429,
    Http5xx,
    HttpOther,
    Timeout,
    ConnectFailed,
    TlsFailed,
    MalformedBody,
    SmtpAccepted,
    SmtpAuthRejected,
    SmtpTransient,
    SmtpPermanent,
}

impl TransportClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Http2xx => "http_2xx",
            Self::Http4xx => "http_4xx",
            Self::Http429 => "http_429",
            Self::Http5xx => "http_5xx",
            Self::HttpOther => "http_other",
            Self::Timeout => "timeout",
            Self::ConnectFailed => "connect_failed",
            Self::TlsFailed => "tls_failed",
            Self::MalformedBody => "malformed_body",
            Self::SmtpAccepted => "smtp_accepted",
            Self::SmtpAuthRejected => "smtp_auth_rejected",
            Self::SmtpTransient => "smtp_transient",
            Self::SmtpPermanent => "smtp_permanent",
        }
    }
}

/// provider 呼出し 1 回の結果 (分類済み)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderOutcome {
    /// 実 verification の business success (有効 token の検証成功など)。
    BusinessSuccess,
    /// 資格情報・接続が受理されたが business 結果ではない (SMTP 250・負の probe で secret が受理された等)。
    Accepted,
    ConfigurationInvalid,
    /// 当該試行だけの拒否。provider 全体を down にしない (観測を変えない)。
    AttemptRejected,
    TemporarilyUnavailable,
}

impl ProviderOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BusinessSuccess => "business_success",
            Self::Accepted => "accepted",
            Self::ConfigurationInvalid => "configuration_invalid",
            Self::AttemptRejected => "attempt_rejected",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderResult {
    pub outcome: ProviderOutcome,
    pub reason: ReasonCode,
    pub transport: TransportClass,
}

impl ProviderResult {
    pub const fn new(
        outcome: ProviderOutcome,
        reason: ReasonCode,
        transport: TransportClass,
    ) -> Self {
        Self {
            outcome,
            reason,
            transport,
        }
    }

    pub const fn business_success(transport: TransportClass) -> Self {
        Self::new(
            ProviderOutcome::BusinessSuccess,
            ReasonCode::BusinessSuccess,
            transport,
        )
    }

    pub const fn accepted(transport: TransportClass) -> Self {
        Self::new(ProviderOutcome::Accepted, ReasonCode::Accepted, transport)
    }

    pub const fn configuration_invalid(reason: ReasonCode, transport: TransportClass) -> Self {
        Self::new(ProviderOutcome::ConfigurationInvalid, reason, transport)
    }

    pub const fn attempt_rejected(reason: ReasonCode, transport: TransportClass) -> Self {
        Self::new(ProviderOutcome::AttemptRejected, reason, transport)
    }

    pub const fn temporarily_unavailable(reason: ReasonCode, transport: TransportClass) -> Self {
        Self::new(ProviderOutcome::TemporarilyUnavailable, reason, transport)
    }
}

/// 検証結果が確認した範囲。メールでは、接続・認証だけを確認する probe と、宛先を伴う配送 (RCPT/DATA) を確認する
/// canary を区別する。**配送範囲の失敗は、接続範囲の成功 (接続 probe の受理) では解除されない。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckScope {
    /// 接続・認証・設定・HTTP provider 全般 (登録者の code 確認の成功も、配送の再検証ではなくここに含める)。
    Connection,
    /// 管理者 canary (固定宛先) の配送。成功すると接続範囲の既知の障害も解消する (接続して配送できた証拠)。
    Delivery,
}

/// 範囲ごとに保持される未復旧の既知の障害 (`configuration_invalid` / `temporarily_unavailable`)。
///
/// **単調性**: より悪い既知状態は、それを解消する証拠 (同じ範囲の成功) か generation の変更でしか改善しない。
/// 後続のより軽い失敗 (例: configuration_invalid の後の timeout)、時間の経過 (鮮度切れ)、他の範囲の成功では
/// 解除も軽減もされない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownFailure {
    pub outcome: ProviderOutcome,
    pub reason: ReasonCode,
    pub transport: TransportClass,
    pub at: OffsetDateTime,
}

impl KnownFailure {
    fn severity(&self) -> u8 {
        match self.outcome {
            ProviderOutcome::ConfigurationInvalid => 2,
            _ => 1,
        }
    }

    fn from_result(result: &ProviderResult, at: OffsetDateTime) -> Option<Self> {
        matches!(
            result.outcome,
            ProviderOutcome::ConfigurationInvalid | ProviderOutcome::TemporarilyUnavailable
        )
        .then_some(Self {
            outcome: result.outcome,
            reason: result.reason,
            transport: result.transport,
            at,
        })
    }

    /// 新しい失敗を既存の同範囲の失敗へ畳む。より軽い失敗では改善しない (同格以上なら最新で置き換える)。
    fn escalate(existing: Option<Self>, new: Self) -> Self {
        match existing {
            Some(existing) if existing.severity() > new.severity() => existing,
            _ => new,
        }
    }
}

/// 保護ごとの有効設定のスナップショット (secret を含まない)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectionConfigSnapshot {
    pub protection: Protection,
    pub provider: ProtectionProvider,
    /// 明示無効・未設定は false。
    pub enabled: bool,
    /// `enabled=false` または設定が検証に使えない (不備)。`Some` なら `configuration_invalid`。
    pub issue: Option<ReasonCode>,
    pub generation: ConfigGeneration,
    /// secret の逆算できない短い fingerprint (非 secret)。secret がなければ空。
    pub fingerprint: String,
}

impl ProtectionConfigSnapshot {
    pub fn is_usable(&self) -> bool {
        self.enabled && self.issue.is_none()
    }
}

/// readiness の鮮度ポリシー。時刻は常に引数で注入する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadinessPolicy {
    /// business success を有効とみなす期間。超過で `unverified` (success_expired)。
    pub success_ttl: Duration,
    /// 最後の検証 (probe) からこの期間を超えたら `unverified` (probe_stale)。
    /// 継続 probe の周期はこれより短くすること (周期実行は E15d)。
    pub probe_max_age: Duration,
}

impl Default for ReadinessPolicy {
    fn default() -> Self {
        Self {
            success_ttl: Duration::hours(24),
            probe_max_age: Duration::minutes(10),
        }
    }
}

/// 永続化される readiness 記録 (保護ごとに 1 行)。
///
/// `status`/`reason`/`valid_until` は記録時点の評価のスナップショットで、**現在の判定は常に**
/// `evaluate_readiness` で再評価する (時刻・設定が変わると変わるため)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessRecord {
    pub protection: Protection,
    pub provider: ProtectionProvider,
    pub enabled: bool,
    pub generation: ConfigGeneration,
    pub fingerprint: String,
    /// 最後の検証の時刻・結果 (business 結果と transport 分類の両方)。
    pub checked_at: OffsetDateTime,
    pub check_outcome: ProviderOutcome,
    pub check_reason: ReasonCode,
    pub check_transport: TransportClass,
    /// 当該 generation での最後の business success とその有効期限。
    pub last_success_at: Option<OffsetDateTime>,
    pub success_valid_until: Option<OffsetDateTime>,
    /// 接続範囲の未復旧の既知の障害 (同一 generation、接続範囲以上の成功でのみ解消)。
    pub connection_failure: Option<KnownFailure>,
    /// 配送範囲 (canary) の未復旧の既知の障害 (同一 generation、canary の配送成功でのみ解消)。
    pub delivery_failure: Option<KnownFailure>,
    pub status: ReadinessStatus,
    pub reason: ReasonCode,
    pub valid_until: Option<OffsetDateTime>,
}

/// 現在時刻での readiness の評価結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveReadiness {
    pub status: ReadinessStatus,
    pub reason: ReasonCode,
    /// 最後の検証時刻 (未検証なら None)。
    pub checked_at: Option<OffsetDateTime>,
    /// operational が続く期限 (operational 以外は None)。
    pub valid_until: Option<OffsetDateTime>,
}

/// readiness を現在の設定・時刻で評価する純粋関数 (契約 §3)。
///
/// 1. 設定が使えない (未設定・空・明示無効・不備) → `configuration_invalid`
/// 2. 記録なし / generation 不一致 → `unverified` (旧 generation の成功は使わない)
/// 3. 最後の検証が `probe_max_age` 以上前 → `unverified` (probe 停止)
/// 4. 最後の検証が configuration_invalid / temporarily_unavailable → その分類
/// 5. 最後の検証が受理 (success/accepted) → 当該 generation の business success が鮮度内なら
///    `operational`、そうでなければ `unverified`
///
/// 期限ちょうど (`now == valid_until`) は期限切れとして扱う。
pub fn evaluate_readiness(
    config: &ProtectionConfigSnapshot,
    record: Option<&ReadinessRecord>,
    now: OffsetDateTime,
    policy: &ReadinessPolicy,
) -> EffectiveReadiness {
    let unverified = |reason, checked_at| EffectiveReadiness {
        status: ReadinessStatus::Unverified,
        reason,
        checked_at,
        valid_until: None,
    };
    if !config.enabled {
        return EffectiveReadiness {
            status: ReadinessStatus::ConfigurationInvalid,
            reason: config.issue.unwrap_or(ReasonCode::Disabled),
            checked_at: record.map(|r| r.checked_at),
            valid_until: None,
        };
    }
    if let Some(issue) = config.issue {
        return EffectiveReadiness {
            status: ReadinessStatus::ConfigurationInvalid,
            reason: issue,
            checked_at: record.map(|r| r.checked_at),
            valid_until: None,
        };
    }
    let Some(record) = record else {
        return unverified(ReasonCode::NeverVerified, None);
    };
    if record.generation != config.generation {
        return unverified(ReasonCode::GenerationChanged, Some(record.checked_at));
    }
    // 既知の障害は鮮度切れでも改善しない (単調性)。最も悪い障害を優先し、同格なら新しい方の理由を使う。
    let worst = [record.connection_failure, record.delivery_failure]
        .into_iter()
        .flatten()
        .max_by_key(|failure| (failure.severity(), failure.at));
    if let Some(failure) = worst {
        return EffectiveReadiness {
            status: if failure.outcome == ProviderOutcome::ConfigurationInvalid {
                ReadinessStatus::ConfigurationInvalid
            } else {
                ReadinessStatus::TemporarilyUnavailable
            },
            reason: failure.reason,
            checked_at: Some(record.checked_at),
            valid_until: None,
        };
    }
    let probe_expires_at = record.checked_at + policy.probe_max_age;
    if now >= probe_expires_at {
        return unverified(ReasonCode::ProbeStale, Some(record.checked_at));
    }
    match record.success_valid_until {
        Some(valid_until) if now < valid_until => EffectiveReadiness {
            status: ReadinessStatus::Operational,
            reason: ReasonCode::BusinessSuccess,
            checked_at: Some(record.checked_at),
            valid_until: Some(valid_until.min(probe_expires_at)),
        },
        Some(_) => unverified(ReasonCode::SuccessExpired, Some(record.checked_at)),
        None => unverified(ReasonCode::NoBusinessSuccess, Some(record.checked_at)),
    }
}

/// 検証結果を既存記録に畳み込んで新しい記録を作る純粋関数。
///
/// - `attempt_rejected` は記録を変えない (`None`)。
/// - 同一 generation なら過去の business success を引き継ぎ、別 generation なら破棄する。
/// - `business_success` は成功時刻と `success_ttl` 後の期限を更新する。
pub fn apply_provider_result(
    existing: Option<&ReadinessRecord>,
    config: &ProtectionConfigSnapshot,
    result: &ProviderResult,
    at: OffsetDateTime,
    policy: &ReadinessPolicy,
) -> Option<ReadinessRecord> {
    apply_scoped_provider_result(existing, config, result, CheckScope::Connection, at, policy)
}

/// `apply_provider_result` の範囲つき版。配送範囲の失敗は `delivery_failure` に保持し、配送範囲の成功
/// (canary の配送受理・確認の business success) でだけ解除する。同一 generation でのみ引き継ぐ。
pub fn apply_scoped_provider_result(
    existing: Option<&ReadinessRecord>,
    config: &ProtectionConfigSnapshot,
    result: &ProviderResult,
    scope: CheckScope,
    at: OffsetDateTime,
    policy: &ReadinessPolicy,
) -> Option<ReadinessRecord> {
    if result.outcome == ProviderOutcome::AttemptRejected {
        return None;
    }
    let carried = existing.filter(|record| record.generation == config.generation);
    let carried_connection = carried.and_then(|record| record.connection_failure);
    let carried_delivery = carried.and_then(|record| record.delivery_failure);
    let (connection_failure, delivery_failure) =
        match (scope, KnownFailure::from_result(result, at)) {
            (CheckScope::Connection, Some(new)) => (
                Some(KnownFailure::escalate(carried_connection, new)),
                carried_delivery,
            ),
            (CheckScope::Delivery, Some(new)) => (
                carried_connection,
                Some(KnownFailure::escalate(carried_delivery, new)),
            ),
            // 成功は同じ範囲の障害だけを解消する。配送の成功は接続できた証拠でもあるので接続範囲も解消する。
            (CheckScope::Connection, None) => (None, carried_delivery),
            (CheckScope::Delivery, None) => (None, None),
        };
    let (last_success_at, success_valid_until) =
        if result.outcome == ProviderOutcome::BusinessSuccess {
            (Some(at), Some(at + policy.success_ttl))
        } else {
            (
                carried.and_then(|record| record.last_success_at),
                carried.and_then(|record| record.success_valid_until),
            )
        };
    let mut record = ReadinessRecord {
        protection: config.protection,
        provider: config.provider,
        enabled: config.enabled,
        generation: config.generation.clone(),
        fingerprint: config.fingerprint.clone(),
        checked_at: at,
        check_outcome: result.outcome,
        check_reason: result.reason,
        check_transport: result.transport,
        last_success_at,
        success_valid_until,
        connection_failure,
        delivery_failure,
        status: ReadinessStatus::Unverified,
        reason: ReasonCode::NeverVerified,
        valid_until: None,
    };
    let effective = evaluate_readiness(config, Some(&record), at, policy);
    record.status = effective.status;
    record.reason = effective.reason;
    record.valid_until = effective.valid_until;
    Some(record)
}

/// 設定が使えない保護の記録 (起動時などに設定喪失を残す用途)。評価は常に `evaluate_readiness` が
/// 設定から直接導くので、記録がなくても `configuration_invalid` になる。
pub fn observation_from_readiness(
    email: &EffectiveReadiness,
    bot: &EffectiveReadiness,
) -> crate::entity::RegistrationObservation {
    crate::entity::RegistrationObservation::new(
        email.status.to_observation(),
        bot.status.to_observation(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

    fn config(generation: &str) -> ProtectionConfigSnapshot {
        ProtectionConfigSnapshot {
            protection: Protection::Bot,
            provider: ProtectionProvider::Turnstile,
            enabled: true,
            issue: None,
            generation: ConfigGeneration::new(generation),
            fingerprint: "fp".to_string(),
        }
    }

    fn policy() -> ReadinessPolicy {
        ReadinessPolicy {
            success_ttl: Duration::hours(1),
            probe_max_age: Duration::minutes(10),
        }
    }

    fn success() -> ProviderResult {
        ProviderResult::business_success(TransportClass::Http2xx)
    }

    fn accepted() -> ProviderResult {
        ProviderResult::accepted(TransportClass::Http2xx)
    }

    fn record(
        existing: Option<&ReadinessRecord>,
        result: ProviderResult,
        at: OffsetDateTime,
    ) -> ReadinessRecord {
        apply_provider_result(existing, &config("g1"), &result, at, &policy()).unwrap()
    }

    fn eval(record: Option<&ReadinessRecord>, now: OffsetDateTime) -> EffectiveReadiness {
        evaluate_readiness(&config("g1"), record, now, &policy())
    }

    #[test]
    fn never_verified_is_unverified_and_unknown() {
        let effective = eval(None, T0);
        assert_eq!(effective.status, ReadinessStatus::Unverified);
        assert_eq!(effective.reason, ReasonCode::NeverVerified);
        assert_eq!(
            effective.status.to_observation(),
            ProtectionObservation::new(true, ProbeStatus::Unknown)
        );
    }

    #[test]
    fn accepted_alone_never_makes_operational() {
        // HTTP 200 / SMTP 250 / 接続成功 (= Accepted) は business success ではない。
        let record = record(None, accepted(), T0);
        let effective = eval(Some(&record), T0 + Duration::seconds(1));
        assert_eq!(effective.status, ReadinessStatus::Unverified);
        assert_eq!(effective.reason, ReasonCode::NoBusinessSuccess);
    }

    #[test]
    fn business_success_is_operational_until_freshness_expires() {
        let record = record(None, success(), T0);
        let effective = eval(Some(&record), T0 + Duration::minutes(1));
        assert_eq!(effective.status, ReadinessStatus::Operational);
        // probe_max_age (10 分) が成功期限 (1 時間) より短いので valid_until は probe 側。
        assert_eq!(effective.valid_until, Some(T0 + Duration::minutes(10)));
        assert_eq!(
            effective.status.to_observation(),
            ProtectionObservation::new(true, ProbeStatus::Healthy)
        );
    }

    #[test]
    fn probe_stopped_discards_the_old_success() {
        let record = record(None, success(), T0);
        let just_before = eval(
            Some(&record),
            T0 + Duration::minutes(10) - Duration::seconds(1),
        );
        assert_eq!(just_before.status, ReadinessStatus::Operational);
        for offset in [Duration::minutes(10), Duration::minutes(11)] {
            let effective = eval(Some(&record), T0 + offset);
            assert_eq!(effective.status, ReadinessStatus::Unverified);
            assert_eq!(effective.reason, ReasonCode::ProbeStale);
        }
    }

    #[test]
    fn continuing_probes_keep_operational_only_while_the_success_is_fresh() {
        let mut current = record(None, success(), T0);
        // 5 分ごとの negative probe (Accepted) は success を引き継ぐ。
        for step in 1..=11 {
            let at = T0 + Duration::minutes(5 * step);
            current = record(Some(&current), accepted(), at);
            let effective = eval(Some(&current), at);
            if at < T0 + Duration::hours(1) {
                assert_eq!(
                    effective.status,
                    ReadinessStatus::Operational,
                    "step {step}"
                );
                assert!(effective.valid_until.unwrap() <= T0 + Duration::hours(1));
            } else {
                // success_ttl (1 時間) を超えたら probe が続いていても operational ではない。
                assert_eq!(effective.status, ReadinessStatus::Unverified, "step {step}");
                assert_eq!(effective.reason, ReasonCode::SuccessExpired, "step {step}");
            }
        }
    }

    #[test]
    fn success_expiry_boundary_is_exclusive() {
        let mut current = record(None, success(), T0);
        let at = T0 + Duration::hours(1) - Duration::minutes(1);
        current = record(Some(&current), accepted(), at);
        assert_eq!(
            eval(Some(&current), at + Duration::seconds(59)).status,
            ReadinessStatus::Operational
        );
        let expired = eval(Some(&current), T0 + Duration::hours(1));
        assert_eq!(expired.status, ReadinessStatus::Unverified);
        assert_eq!(expired.reason, ReasonCode::SuccessExpired);
    }

    #[test]
    fn generation_change_invalidates_old_success() {
        let record = record(None, success(), T0);
        let changed = evaluate_readiness(&config("g2"), Some(&record), T0, &policy());
        assert_eq!(changed.status, ReadinessStatus::Unverified);
        assert_eq!(changed.reason, ReasonCode::GenerationChanged);
    }

    #[test]
    fn a_new_generation_check_does_not_inherit_the_old_success() {
        let old = record(None, success(), T0);
        let next = apply_provider_result(
            Some(&old),
            &config("g2"),
            &accepted(),
            T0 + Duration::minutes(1),
            &policy(),
        )
        .unwrap();
        assert_eq!(next.last_success_at, None);
        let effective = evaluate_readiness(
            &config("g2"),
            Some(&next),
            T0 + Duration::minutes(1),
            &policy(),
        );
        assert_eq!(effective.reason, ReasonCode::NoBusinessSuccess);
    }

    #[test]
    fn failures_are_classified_and_do_not_erase_the_success() {
        let ok = record(None, success(), T0);
        let down = record(
            Some(&ok),
            ProviderResult::temporarily_unavailable(
                ReasonCode::ProviderServerError,
                TransportClass::Http5xx,
            ),
            T0 + Duration::minutes(1),
        );
        let effective = eval(Some(&down), T0 + Duration::minutes(1));
        assert_eq!(effective.status, ReadinessStatus::TemporarilyUnavailable);
        assert_eq!(effective.reason, ReasonCode::ProviderServerError);
        assert_eq!(
            effective.status.to_observation(),
            ProtectionObservation::new(true, ProbeStatus::Unhealthy)
        );
        // 復旧 (negative probe の受理) で、success が鮮度内なら自動的に operational へ戻る。
        let recovered = record(Some(&down), accepted(), T0 + Duration::minutes(2));
        assert_eq!(
            eval(Some(&recovered), T0 + Duration::minutes(2)).status,
            ReadinessStatus::Operational
        );
    }

    #[test]
    fn configuration_invalid_loses_the_protection_and_recovers_by_reconfiguration() {
        let ok = record(None, success(), T0);
        let invalid = record(
            Some(&ok),
            ProviderResult::configuration_invalid(
                ReasonCode::InvalidSecret,
                TransportClass::Http4xx,
            ),
            T0 + Duration::minutes(1),
        );
        let effective = eval(Some(&invalid), T0 + Duration::minutes(1));
        assert_eq!(effective.status, ReadinessStatus::ConfigurationInvalid);
        assert_eq!(
            effective.status.to_observation(),
            ProtectionObservation::UNCONFIGURED
        );
        assert_eq!(effective.reason, ReasonCode::InvalidSecret);
    }

    #[test]
    fn unconfigured_empty_or_disabled_configuration_is_configuration_invalid() {
        for (enabled, issue, expected) in [
            (
                false,
                Some(ReasonCode::NotConfigured),
                ReasonCode::NotConfigured,
            ),
            (false, Some(ReasonCode::Disabled), ReasonCode::Disabled),
            (
                true,
                Some(ReasonCode::MissingSecret),
                ReasonCode::MissingSecret,
            ),
        ] {
            let mut config = config("g1");
            config.enabled = enabled;
            config.issue = issue;
            // 過去に成功していても、設定が使えなければ旧成功は使わない。
            let old = record(None, success(), T0);
            let effective = evaluate_readiness(&config, Some(&old), T0, &policy());
            assert_eq!(effective.status, ReadinessStatus::ConfigurationInvalid);
            assert_eq!(effective.reason, expected);
        }
    }

    #[test]
    fn attempt_rejected_changes_nothing() {
        let ok = record(None, success(), T0);
        let rejected =
            ProviderResult::attempt_rejected(ReasonCode::TokenRejected, TransportClass::Http2xx);
        assert!(apply_provider_result(
            Some(&ok),
            &config("g1"),
            &rejected,
            T0 + Duration::minutes(1),
            &policy()
        )
        .is_none());
        assert!(apply_provider_result(None, &config("g1"), &rejected, T0, &policy()).is_none());
    }

    #[test]
    fn a_stale_failure_does_not_improve_without_evidence() {
        let invalid = record(
            None,
            ProviderResult::configuration_invalid(
                ReasonCode::InvalidSecret,
                TransportClass::Http4xx,
            ),
            T0,
        );
        let later = eval(Some(&invalid), T0 + Duration::minutes(30));
        assert_eq!(later.status, ReadinessStatus::ConfigurationInvalid);
        assert_eq!(later.reason, ReasonCode::InvalidSecret);
        let recovered = record(Some(&invalid), success(), T0 + Duration::minutes(31));
        assert_eq!(
            eval(Some(&recovered), T0 + Duration::minutes(32)).status,
            ReadinessStatus::Operational
        );
    }

    #[derive(Clone, Copy, Debug)]
    enum Held {
        None,
        Temporary,
        Invalid,
    }

    #[derive(Clone, Copy, Debug)]
    enum Latest {
        Accepted,
        Temporary,
        Invalid,
        NotObserved,
    }

    #[derive(Clone, Copy, Debug)]
    enum When {
        Fresh,
        Stale,
        GenerationChanged,
    }

    const OP: (ReadinessStatus, ReasonCode) =
        (ReadinessStatus::Operational, ReasonCode::BusinessSuccess);
    const T_HELD: (ReadinessStatus, ReasonCode) = (
        ReadinessStatus::TemporarilyUnavailable,
        ReasonCode::SmtpTransientFailure,
    );
    const T_LATEST: (ReadinessStatus, ReasonCode) =
        (ReadinessStatus::TemporarilyUnavailable, ReasonCode::Timeout);
    const C_HELD: (ReadinessStatus, ReasonCode) = (
        ReadinessStatus::ConfigurationInvalid,
        ReasonCode::SmtpPermanentFailure,
    );
    const C_LATEST: (ReadinessStatus, ReasonCode) = (
        ReadinessStatus::ConfigurationInvalid,
        ReasonCode::InvalidSecret,
    );
    const STALE: (ReadinessStatus, ReasonCode) =
        (ReadinessStatus::Unverified, ReasonCode::ProbeStale);
    const GEN: (ReadinessStatus, ReasonCode) =
        (ReadinessStatus::Unverified, ReasonCode::GenerationChanged);

    /// 保持している配送範囲の障害 × 最新の接続結果 × 時間・generation の全組合せ。
    /// 期待値は評価関数から導かず、単調性の原則 (より悪い既知状態は、それを解消する同じ範囲の成功か
    /// generation の変更でしか改善しない) から手で書いている。
    #[test]
    fn readiness_is_monotonic_over_every_combination() {
        use Held::*;
        use Latest::{Accepted as Acc, Invalid as Inv, NotObserved as NoObs, Temporary as Temp};
        use When::*;
        #[rustfmt::skip]
        let table: [(Held, Latest, When, (ReadinessStatus, ReasonCode)); 36] = [
            (None, Acc, Fresh, OP),            (None, Temp, Fresh, T_LATEST),
            (None, Inv, Fresh, C_LATEST),      (None, NoObs, Fresh, OP),
            (Temporary, Acc, Fresh, T_HELD),   (Temporary, Temp, Fresh, T_LATEST),
            (Temporary, Inv, Fresh, C_LATEST), (Temporary, NoObs, Fresh, T_HELD),
            (Invalid, Acc, Fresh, C_HELD),     (Invalid, Temp, Fresh, C_HELD),
            (Invalid, Inv, Fresh, C_LATEST),   (Invalid, NoObs, Fresh, C_HELD),

            (None, Acc, Stale, STALE),         (None, Temp, Stale, T_LATEST),
            (None, Inv, Stale, C_LATEST),      (None, NoObs, Stale, STALE),
            (Temporary, Acc, Stale, T_HELD),   (Temporary, Temp, Stale, T_LATEST),
            (Temporary, Inv, Stale, C_LATEST), (Temporary, NoObs, Stale, T_HELD),
            (Invalid, Acc, Stale, C_HELD),     (Invalid, Temp, Stale, C_HELD),
            (Invalid, Inv, Stale, C_LATEST),   (Invalid, NoObs, Stale, C_HELD),

            (None, Acc, GenerationChanged, GEN),       (None, Temp, GenerationChanged, GEN),
            (None, Inv, GenerationChanged, GEN),       (None, NoObs, GenerationChanged, GEN),
            (Temporary, Acc, GenerationChanged, GEN),  (Temporary, Temp, GenerationChanged, GEN),
            (Temporary, Inv, GenerationChanged, GEN),  (Temporary, NoObs, GenerationChanged, GEN),
            (Invalid, Acc, GenerationChanged, GEN),    (Invalid, Temp, GenerationChanged, GEN),
            (Invalid, Inv, GenerationChanged, GEN),    (Invalid, NoObs, GenerationChanged, GEN),
        ];
        for (held, latest, when, (status, reason)) in table {
            let apply = |existing: Option<&ReadinessRecord>,
                         result: ProviderResult,
                         scope: CheckScope,
                         at: OffsetDateTime| {
                apply_scoped_provider_result(existing, &config("g1"), &result, scope, at, &policy())
                    .unwrap()
            };
            let mut current = apply(Option::None, success(), CheckScope::Delivery, T0);
            match held {
                None => {}
                Temporary => {
                    current = apply(
                        Some(&current),
                        ProviderResult::temporarily_unavailable(
                            ReasonCode::SmtpTransientFailure,
                            TransportClass::SmtpTransient,
                        ),
                        CheckScope::Delivery,
                        T0 + Duration::minutes(1),
                    );
                }
                Invalid => {
                    current = apply(
                        Some(&current),
                        ProviderResult::configuration_invalid(
                            ReasonCode::SmtpPermanentFailure,
                            TransportClass::SmtpPermanent,
                        ),
                        CheckScope::Delivery,
                        T0 + Duration::minutes(1),
                    );
                }
            }
            let latest_result = match latest {
                Latest::Accepted => Some(accepted()),
                Latest::Temporary => Some(ProviderResult::temporarily_unavailable(
                    ReasonCode::Timeout,
                    TransportClass::Timeout,
                )),
                Latest::Invalid => Some(ProviderResult::configuration_invalid(
                    ReasonCode::InvalidSecret,
                    TransportClass::Http4xx,
                )),
                Latest::NotObserved => Option::None,
            };
            if let Some(result) = latest_result {
                current = apply(
                    Some(&current),
                    result,
                    CheckScope::Connection,
                    T0 + Duration::minutes(2),
                );
            }
            let (config, now) = match when {
                Fresh => (config("g1"), T0 + Duration::minutes(3)),
                Stale => (config("g1"), T0 + Duration::minutes(30)),
                GenerationChanged => (config("g2"), T0 + Duration::minutes(3)),
            };
            let effective = evaluate_readiness(&config, Some(&current), now, &policy());
            let case = format!("{held:?} x {latest:?} x {when:?}");
            assert_eq!(effective.status, status, "{case}");
            assert_eq!(effective.reason, reason, "{case}");
            let expected_observation = match status {
                ReadinessStatus::Operational => {
                    ProtectionObservation::new(true, ProbeStatus::Healthy)
                }
                ReadinessStatus::TemporarilyUnavailable => {
                    ProtectionObservation::new(true, ProbeStatus::Unhealthy)
                }
                ReadinessStatus::Unverified => {
                    ProtectionObservation::new(true, ProbeStatus::Unknown)
                }
                ReadinessStatus::ConfigurationInvalid => ProtectionObservation::UNCONFIGURED,
            };
            assert_eq!(
                effective.status.to_observation(),
                expected_observation,
                "{case}"
            );
        }
    }

    #[test]
    fn a_failure_is_cleared_only_by_a_success_of_the_same_scope() {
        let apply = |existing: Option<&ReadinessRecord>,
                     result: ProviderResult,
                     scope: CheckScope,
                     minute: i64| {
            apply_scoped_provider_result(
                existing,
                &config("g1"),
                &result,
                scope,
                T0 + Duration::minutes(minute),
                &policy(),
            )
            .unwrap()
        };
        let timeout = || {
            ProviderResult::temporarily_unavailable(ReasonCode::Timeout, TransportClass::Timeout)
        };
        let auth_failed = || {
            ProviderResult::configuration_invalid(
                ReasonCode::SmtpAuthFailed,
                TransportClass::SmtpPermanent,
            )
        };
        let rcpt_refused = || {
            ProviderResult::configuration_invalid(
                ReasonCode::SmtpPermanentFailure,
                TransportClass::SmtpPermanent,
            )
        };
        let status = |record: &ReadinessRecord, minute: i64| {
            eval(Some(record), T0 + Duration::minutes(minute)).status
        };
        let ok = apply(None, success(), CheckScope::Delivery, 0);

        // 接続範囲の configuration_invalid は、より軽い失敗 (timeout) では軽くならず、接続範囲の成功で解消する。
        let invalid = apply(Some(&ok), auth_failed(), CheckScope::Connection, 1);
        let after_timeout = apply(Some(&invalid), timeout(), CheckScope::Connection, 2);
        assert_eq!(
            status(&after_timeout, 2),
            ReadinessStatus::ConfigurationInvalid
        );
        assert_eq!(
            eval(Some(&after_timeout), T0 + Duration::minutes(2)).reason,
            ReasonCode::SmtpAuthFailed
        );
        let healed = apply(Some(&after_timeout), accepted(), CheckScope::Connection, 3);
        assert_eq!(status(&healed, 3), ReadinessStatus::Operational);

        // 配送範囲の configuration_invalid は、接続範囲の成功・より軽い配送失敗では解消も軽減もしない。
        let blocked = apply(Some(&ok), rcpt_refused(), CheckScope::Delivery, 1);
        let by_connection = apply(Some(&blocked), success(), CheckScope::Connection, 2);
        assert_eq!(
            status(&by_connection, 2),
            ReadinessStatus::ConfigurationInvalid
        );
        let by_lighter = apply(Some(&by_connection), timeout(), CheckScope::Delivery, 3);
        assert_eq!(
            status(&by_lighter, 3),
            ReadinessStatus::ConfigurationInvalid
        );
        assert_eq!(
            eval(Some(&by_lighter), T0 + Duration::minutes(3)).reason,
            ReasonCode::SmtpPermanentFailure
        );
        // 配送範囲の成功は接続範囲の障害も解消する (接続して配送できた証拠)。
        let both = apply(Some(&by_lighter), auth_failed(), CheckScope::Connection, 4);
        let healed = apply(Some(&both), success(), CheckScope::Delivery, 5);
        assert!(healed.connection_failure.is_none() && healed.delivery_failure.is_none());
        assert_eq!(status(&healed, 5), ReadinessStatus::Operational);
    }

    #[test]
    fn observation_is_derived_per_protection_independently() {
        let bot_down = EffectiveReadiness {
            status: ReadinessStatus::ConfigurationInvalid,
            reason: ReasonCode::InvalidSecret,
            checked_at: None,
            valid_until: None,
        };
        let email_ok = EffectiveReadiness {
            status: ReadinessStatus::Operational,
            reason: ReasonCode::BusinessSuccess,
            checked_at: Some(T0),
            valid_until: Some(T0),
        };
        let observation = observation_from_readiness(&email_ok, &bot_down);
        assert!(observation.email.is_effective());
        assert!(!observation.bot.is_effective());
        assert!(!observation.bot.configured);
    }

    #[test]
    fn a_delivery_failure_is_not_cleared_by_a_connection_probe_only_by_a_delivery_success() {
        let apply = |existing: Option<&ReadinessRecord>,
                     result: ProviderResult,
                     scope: CheckScope,
                     at: OffsetDateTime| {
            apply_scoped_provider_result(existing, &config("g1"), &result, scope, at, &policy())
                .unwrap()
        };
        let delivery_temp = ProviderResult::temporarily_unavailable(
            ReasonCode::SmtpTransientFailure,
            TransportClass::SmtpTransient,
        );
        let delivery_invalid = ProviderResult::configuration_invalid(
            ReasonCode::SmtpPermanentFailure,
            TransportClass::SmtpPermanent,
        );
        let ok = apply(None, success(), CheckScope::Delivery, T0);

        for (failure, expected, observation) in [
            (
                delivery_temp,
                ReadinessStatus::TemporarilyUnavailable,
                ProtectionObservation::new(true, ProbeStatus::Unhealthy),
            ),
            (
                delivery_invalid,
                ReadinessStatus::ConfigurationInvalid,
                ProtectionObservation::UNCONFIGURED,
            ),
        ] {
            let failed = apply(
                Some(&ok),
                failure,
                CheckScope::Delivery,
                T0 + Duration::minutes(1),
            );
            assert_eq!(
                eval(Some(&failed), T0 + Duration::minutes(1)).status,
                expected
            );
            // 障害継続中の接続 probe の成功 (Accepted) では operational へ戻らない。繰り返しても同じ。
            let mut current = failed;
            for step in 2..=5 {
                let at = T0 + Duration::minutes(step);
                current = apply(Some(&current), accepted(), CheckScope::Connection, at);
                let effective = eval(Some(&current), at);
                assert_eq!(effective.status, expected, "step {step}");
                assert_eq!(
                    effective.status.to_observation(),
                    observation,
                    "step {step}"
                );
                assert_eq!(effective.reason, failure.reason, "step {step}");
            }
            // 配送範囲の成功 (canary の配送受理) で初めて解除され、鮮度内の business success で operational。
            let at = T0 + Duration::minutes(6);
            let recovered = apply(Some(&current), accepted(), CheckScope::Delivery, at);
            assert!(recovered.delivery_failure.is_none());
            assert_eq!(
                eval(Some(&recovered), at).status,
                ReadinessStatus::Operational
            );
        }
    }

    #[test]
    fn a_connection_level_failure_still_recovers_with_a_connection_probe() {
        let ok = record(None, success(), T0);
        let down = record(
            Some(&ok),
            ProviderResult::temporarily_unavailable(
                ReasonCode::ConnectFailed,
                TransportClass::ConnectFailed,
            ),
            T0 + Duration::minutes(1),
        );
        assert_eq!(
            eval(Some(&down), T0 + Duration::minutes(1)).status,
            ReadinessStatus::TemporarilyUnavailable
        );
        assert!(down.delivery_failure.is_none());
        let recovered = record(Some(&down), accepted(), T0 + Duration::minutes(2));
        assert_eq!(
            eval(Some(&recovered), T0 + Duration::minutes(2)).status,
            ReadinessStatus::Operational
        );
    }

    #[test]
    fn a_delivery_failure_does_not_survive_a_configuration_change() {
        let failed = apply_scoped_provider_result(
            None,
            &config("g1"),
            &ProviderResult::temporarily_unavailable(
                ReasonCode::SmtpTransientFailure,
                TransportClass::SmtpTransient,
            ),
            CheckScope::Delivery,
            T0,
            &policy(),
        )
        .unwrap();
        assert!(failed.delivery_failure.is_some());
        let next = apply_scoped_provider_result(
            Some(&failed),
            &config("g2"),
            &accepted(),
            CheckScope::Connection,
            T0 + Duration::minutes(1),
            &policy(),
        )
        .unwrap();
        assert!(next.delivery_failure.is_none());
    }

    #[test]
    fn snapshot_record_status_matches_the_evaluation_at_check_time() {
        let ok = record(None, success(), T0);
        assert_eq!(ok.status, ReadinessStatus::Operational);
        assert_eq!(ok.valid_until, Some(T0 + Duration::minutes(10)));
        let accepted_only = record(None, accepted(), T0);
        assert_eq!(accepted_only.status, ReadinessStatus::Unverified);
    }
}
