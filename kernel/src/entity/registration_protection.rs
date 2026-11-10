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
    let probe_expires_at = record.checked_at + policy.probe_max_age;
    if now >= probe_expires_at {
        return unverified(ReasonCode::ProbeStale, Some(record.checked_at));
    }
    match record.check_outcome {
        ProviderOutcome::ConfigurationInvalid => EffectiveReadiness {
            status: ReadinessStatus::ConfigurationInvalid,
            reason: record.check_reason,
            checked_at: Some(record.checked_at),
            valid_until: None,
        },
        ProviderOutcome::TemporarilyUnavailable => EffectiveReadiness {
            status: ReadinessStatus::TemporarilyUnavailable,
            reason: record.check_reason,
            checked_at: Some(record.checked_at),
            valid_until: None,
        },
        // attempt_rejected は記録されないが、万一あっても観測を変えない (受理扱いにはしない)。
        ProviderOutcome::BusinessSuccess
        | ProviderOutcome::Accepted
        | ProviderOutcome::AttemptRejected => match record.success_valid_until {
            Some(valid_until) if now < valid_until => EffectiveReadiness {
                status: ReadinessStatus::Operational,
                reason: ReasonCode::BusinessSuccess,
                checked_at: Some(record.checked_at),
                valid_until: Some(valid_until.min(probe_expires_at)),
            },
            Some(_) => unverified(ReasonCode::SuccessExpired, Some(record.checked_at)),
            None => unverified(ReasonCode::NoBusinessSuccess, Some(record.checked_at)),
        },
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
    if result.outcome == ProviderOutcome::AttemptRejected {
        return None;
    }
    let carried = existing.filter(|record| record.generation == config.generation);
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
    fn stale_failure_becomes_unverified_instead_of_pinning_the_failure_forever() {
        let invalid = record(
            None,
            ProviderResult::configuration_invalid(
                ReasonCode::InvalidSecret,
                TransportClass::Http4xx,
            ),
            T0,
        );
        let later = eval(Some(&invalid), T0 + Duration::minutes(30));
        assert_eq!(later.status, ReadinessStatus::Unverified);
        assert_eq!(later.reason, ReasonCode::ProbeStale);
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
    fn snapshot_record_status_matches_the_evaluation_at_check_time() {
        let ok = record(None, success(), T0);
        assert_eq!(ok.status, ReadinessStatus::Operational);
        assert_eq!(ok.valid_until, Some(T0 + Duration::minutes(10)));
        let accepted_only = record(None, accepted(), T0);
        assert_eq!(accepted_only.status, ReadinessStatus::Unverified);
    }
}
