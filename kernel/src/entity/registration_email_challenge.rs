//! 登録前 email challenge / proof (D15 / E15c) のドメイン。
//!
//! 正規化 email + 実 registration flow ID + config generation に challenge を束縛し、code の確認で
//! 所有を検証した後にだけ opaque proof を発行する。DB に残すのは code / proof の **hash** と
//! email の hash だけで、平文は持たない。ここには I/O のない純粋ロジック (正規化・送信 rate limit・
//! 確認/消費の判定) を置く。時刻は常に引数。

use crate::entity::ConfigGeneration;
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

/// 正規化済み email (trim・小文字化。ASCII のみ)。Kratos の identifier 正規化 (小文字) と揃える。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NormalizedEmail(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmailFormatError {
    Empty,
    TooLong,
    NotAscii,
    Malformed,
}

pub const MAX_EMAIL_LEN: usize = 254;

impl NormalizedEmail {
    pub fn parse(raw: &str) -> Result<Self, EmailFormatError> {
        let value = raw.trim().to_ascii_lowercase();
        if value.is_empty() {
            return Err(EmailFormatError::Empty);
        }
        if value.len() > MAX_EMAIL_LEN {
            return Err(EmailFormatError::TooLong);
        }
        if !value.is_ascii() {
            return Err(EmailFormatError::NotAscii);
        }
        // SMTP コマンド/ヘッダー注入と複数宛先を防ぐため、空白・制御文字・区切り記号を許さない。
        if value
            .bytes()
            .any(|b| b.is_ascii_control() || b == b' ' || b"<>(),;:\\\"[]".contains(&b))
        {
            return Err(EmailFormatError::Malformed);
        }
        let mut parts = value.split('@');
        let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(EmailFormatError::Malformed);
        };
        let domain_ok = domain.contains('.')
            && !domain.starts_with('.')
            && !domain.ends_with('.')
            && !domain.contains("..")
            && !domain.starts_with('-')
            && domain
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        if local.is_empty()
            || local.len() > 64
            || local.starts_with('.')
            || local.ends_with('.')
            || local.contains("..")
            || !domain_ok
        {
            return Err(EmailFormatError::Malformed);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChallengePurpose {
    /// 登録者の所有確認。確認後に proof を発行する。
    Registration,
    /// 管理者の readiness 検証 (承認済み canary mailbox)。proof は発行せず、他の登録者の
    /// 所有証明にならない。
    Canary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeStatus {
    /// 確認待ち (送信失敗・試行超過・世代変更では `Invalidated`)。
    Pending,
    Confirmed,
    Invalidated,
}

/// 保存される challenge。code / proof / email は hash のみ (hex)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailChallenge {
    /// 推測困難な opaque ID (128 bit 乱数の hex)。
    pub id: String,
    pub purpose: ChallengePurpose,
    pub flow_id: String,
    pub email_hash: String,
    pub generation: ConfigGeneration,
    pub code_hash: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub status: ChallengeStatus,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub flow_expires_at: OffsetDateTime,
    pub confirmed_at: Option<OffsetDateTime>,
    pub proof_hash: Option<String>,
    pub proof_expires_at: Option<OffsetDateTime>,
    pub consumed_at: Option<OffsetDateTime>,
}

/// TTL・試行上限・rate limit の既定値 (根拠は docs/registration-protection-readiness.md)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmailChallengePolicy {
    pub challenge_ttl: Duration,
    pub max_attempts: i32,
    pub proof_ttl: Duration,
    /// Kratos の transport retry にだけ同じ検証済み事実を返す短い再利用窓。
    pub proof_reuse_window: Duration,
    pub limits: SendLimits,
}

impl Default for EmailChallengePolicy {
    fn default() -> Self {
        Self {
            challenge_ttl: Duration::minutes(10),
            max_attempts: 5,
            proof_ttl: Duration::minutes(15),
            proof_reuse_window: Duration::seconds(60),
            limits: SendLimits::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendLimits {
    /// 同一 email への最小送信間隔。
    pub email_interval: Duration,
    pub email_per_hour: i64,
    pub flow_per_hour: i64,
    pub source_per_hour: i64,
}

impl Default for SendLimits {
    fn default() -> Self {
        Self {
            email_interval: Duration::seconds(60),
            email_per_hour: 5,
            flow_per_hour: 3,
            source_per_hour: 20,
        }
    }
}

/// 直近の送信実績 (集計は実 DB 側)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SendCounts {
    pub email_last_sent_at: Option<OffsetDateTime>,
    pub email_in_hour: i64,
    pub flow_in_hour: i64,
    pub source_in_hour: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendLimitScope {
    EmailInterval,
    EmailHourly,
    FlowHourly,
    SourceHourly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendLimitDecision {
    Allowed,
    Limited {
        scope: SendLimitScope,
        retry_after: Duration,
    },
}

pub const RATE_WINDOW: Duration = Duration::hours(1);

/// 送信 rate limit の判定 (純粋)。window の長さ分の再試行待ち時間を保守的に返す。
pub fn evaluate_send_limits(
    counts: &SendCounts,
    limits: &SendLimits,
    now: OffsetDateTime,
) -> SendLimitDecision {
    if let Some(last) = counts.email_last_sent_at {
        let next = last + limits.email_interval;
        if now < next {
            return SendLimitDecision::Limited {
                scope: SendLimitScope::EmailInterval,
                retry_after: next - now,
            };
        }
    }
    let hourly = [
        (
            counts.email_in_hour,
            limits.email_per_hour,
            SendLimitScope::EmailHourly,
        ),
        (
            counts.flow_in_hour,
            limits.flow_per_hour,
            SendLimitScope::FlowHourly,
        ),
        (
            counts.source_in_hour,
            limits.source_per_hour,
            SendLimitScope::SourceHourly,
        ),
    ];
    for (count, limit, scope) in hourly {
        if count >= limit {
            return SendLimitDecision::Limited {
                scope,
                retry_after: RATE_WINDOW,
            };
        }
    }
    SendLimitDecision::Allowed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeRejection {
    NotPending,
    Expired,
    GenerationChanged,
    AttemptsExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmDecision {
    Accept,
    Reject(ChallengeRejection),
    /// code 不一致。試行を 1 回消費し、上限に達したら challenge を失効させる。
    WrongCode {
        attempts: i32,
        invalidate: bool,
    },
}

/// code 確認の判定 (純粋)。`code_matches` は呼び出し側が定数時間比較で求める。
/// 期限ちょうど (`now == expires_at`) は期限切れ。
pub fn decide_confirmation(
    challenge: &EmailChallenge,
    current_generation: &ConfigGeneration,
    code_matches: bool,
    now: OffsetDateTime,
) -> ConfirmDecision {
    if challenge.status != ChallengeStatus::Pending {
        return ConfirmDecision::Reject(ChallengeRejection::NotPending);
    }
    if now >= challenge.expires_at || now >= challenge.flow_expires_at {
        return ConfirmDecision::Reject(ChallengeRejection::Expired);
    }
    if &challenge.generation != current_generation {
        return ConfirmDecision::Reject(ChallengeRejection::GenerationChanged);
    }
    if challenge.attempts >= challenge.max_attempts {
        return ConfirmDecision::Reject(ChallengeRejection::AttemptsExhausted);
    }
    if code_matches {
        return ConfirmDecision::Accept;
    }
    let attempts = challenge.attempts + 1;
    ConfirmDecision::WrongCode {
        attempts,
        invalidate: attempts >= challenge.max_attempts,
    }
}

/// proof が束縛されるべき実 context (hook が実 flow/email から導く)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofBinding {
    pub flow_id: String,
    pub email_hash: String,
    pub generation: ConfigGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofRejection {
    /// 該当する proof がない (改変・未発行・canary など)。
    Unknown,
    FlowMismatch,
    EmailMismatch,
    GenerationMismatch,
    Expired,
    /// 既に消費済みで再利用窓の外 (replay)。
    AlreadyConsumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofDecision {
    /// 初回の消費 (原子的に消費済みへ更新する)。
    Consume,
    /// 同一 binding の transport retry (再利用窓内)。同じ検証済み事実を返す。
    Replay,
    Reject(ProofRejection),
}

/// proof の照合・消費の判定 (純粋)。gate の許可 verdict は含まない (毎回最新評価は E15d)。
pub fn decide_proof_use(
    challenge: Option<&EmailChallenge>,
    expected: &ProofBinding,
    now: OffsetDateTime,
    reuse_window: Duration,
) -> ProofDecision {
    let Some(challenge) = challenge else {
        return ProofDecision::Reject(ProofRejection::Unknown);
    };
    if challenge.purpose != ChallengePurpose::Registration
        || challenge.status != ChallengeStatus::Confirmed
        || challenge.proof_hash.is_none()
    {
        return ProofDecision::Reject(ProofRejection::Unknown);
    }
    if challenge.flow_id != expected.flow_id {
        return ProofDecision::Reject(ProofRejection::FlowMismatch);
    }
    if challenge.email_hash != expected.email_hash {
        return ProofDecision::Reject(ProofRejection::EmailMismatch);
    }
    if challenge.generation != expected.generation {
        return ProofDecision::Reject(ProofRejection::GenerationMismatch);
    }
    if let Some(consumed_at) = challenge.consumed_at {
        return if now >= consumed_at && now < consumed_at + reuse_window {
            ProofDecision::Replay
        } else {
            ProofDecision::Reject(ProofRejection::AlreadyConsumed)
        };
    }
    match challenge.proof_expires_at {
        Some(expires_at) if now < expires_at && now < challenge.flow_expires_at => {
            ProofDecision::Consume
        }
        _ => ProofDecision::Reject(ProofRejection::Expired),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

    fn generation(value: &str) -> ConfigGeneration {
        ConfigGeneration::new(value)
    }

    fn challenge() -> EmailChallenge {
        EmailChallenge {
            id: "c1".into(),
            purpose: ChallengePurpose::Registration,
            flow_id: "flow-1".into(),
            email_hash: "email-1".into(),
            generation: generation("g1"),
            code_hash: "code".into(),
            attempts: 0,
            max_attempts: 3,
            status: ChallengeStatus::Pending,
            created_at: T0,
            expires_at: T0 + Duration::minutes(10),
            flow_expires_at: T0 + Duration::minutes(30),
            confirmed_at: None,
            proof_hash: None,
            proof_expires_at: None,
            consumed_at: None,
        }
    }

    fn confirmed() -> EmailChallenge {
        EmailChallenge {
            status: ChallengeStatus::Confirmed,
            confirmed_at: Some(T0 + Duration::minutes(1)),
            proof_hash: Some("proof".into()),
            proof_expires_at: Some(T0 + Duration::minutes(16)),
            ..challenge()
        }
    }

    fn binding() -> ProofBinding {
        ProofBinding {
            flow_id: "flow-1".into(),
            email_hash: "email-1".into(),
            generation: generation("g1"),
        }
    }

    #[test]
    fn email_is_trimmed_lowercased_and_validated() {
        assert_eq!(
            NormalizedEmail::parse("  Alice.Smith@Example.COM ")
                .unwrap()
                .as_str(),
            "alice.smith@example.com"
        );
        for bad in [
            "",
            "   ",
            "no-at-sign",
            "a@b",
            "a@@example.com",
            "a b@example.com",
            "a@example.com,b@example.com",
            "a@example.com;b@example.com",
            "<a@example.com>",
            "a@example.com\r\nBcc: x@example.com",
            "\"a\"@example.com",
            ".a@example.com",
            "a..b@example.com",
            "a@.example.com",
            "a@example..com",
            "a@-example.com",
            "日本語@example.com",
            "a@exämple.com",
        ] {
            assert!(
                NormalizedEmail::parse(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
        let long_local = format!("{}@example.com", "a".repeat(65));
        assert!(NormalizedEmail::parse(&long_local).is_err());
        let too_long = format!("a@{}.com", "b".repeat(260));
        assert_eq!(
            NormalizedEmail::parse(&too_long),
            Err(EmailFormatError::TooLong)
        );
    }

    #[test]
    fn send_limits_cover_interval_hourly_flow_and_source() {
        let limits = SendLimits::default();
        let counts = SendCounts::default();
        assert_eq!(
            evaluate_send_limits(&counts, &limits, T0),
            SendLimitDecision::Allowed
        );

        let recent = SendCounts {
            email_last_sent_at: Some(T0 - Duration::seconds(59)),
            ..counts
        };
        assert_eq!(
            evaluate_send_limits(&recent, &limits, T0),
            SendLimitDecision::Limited {
                scope: SendLimitScope::EmailInterval,
                retry_after: Duration::seconds(1)
            }
        );
        let exactly_interval = SendCounts {
            email_last_sent_at: Some(T0 - Duration::seconds(60)),
            ..counts
        };
        assert_eq!(
            evaluate_send_limits(&exactly_interval, &limits, T0),
            SendLimitDecision::Allowed
        );

        for (counts, scope) in [
            (
                SendCounts {
                    email_in_hour: 5,
                    ..counts
                },
                SendLimitScope::EmailHourly,
            ),
            (
                SendCounts {
                    flow_in_hour: 3,
                    ..counts
                },
                SendLimitScope::FlowHourly,
            ),
            (
                SendCounts {
                    source_in_hour: 20,
                    ..counts
                },
                SendLimitScope::SourceHourly,
            ),
        ] {
            assert!(matches!(
                evaluate_send_limits(&counts, &limits, T0),
                SendLimitDecision::Limited { scope: s, .. } if s == scope
            ));
        }
        let under = SendCounts {
            email_in_hour: 4,
            flow_in_hour: 2,
            source_in_hour: 19,
            ..counts
        };
        assert_eq!(
            evaluate_send_limits(&under, &limits, T0),
            SendLimitDecision::Allowed
        );
    }

    #[test]
    fn confirmation_boundaries_attempts_and_generation() {
        let g = generation("g1");
        let c = challenge();
        assert_eq!(
            decide_confirmation(&c, &g, true, T0),
            ConfirmDecision::Accept
        );
        // 期限直前は有効、期限ちょうどは期限切れ。
        assert_eq!(
            decide_confirmation(&c, &g, true, c.expires_at - Duration::seconds(1)),
            ConfirmDecision::Accept
        );
        assert_eq!(
            decide_confirmation(&c, &g, true, c.expires_at),
            ConfirmDecision::Reject(ChallengeRejection::Expired)
        );
        assert_eq!(
            decide_confirmation(&c, &g, true, c.expires_at + Duration::seconds(1)),
            ConfirmDecision::Reject(ChallengeRejection::Expired)
        );
        assert_eq!(
            decide_confirmation(&c, &generation("g2"), true, T0),
            ConfirmDecision::Reject(ChallengeRejection::GenerationChanged)
        );
        // flow が先に切れるなら flow 期限で打ち切る。
        let short_flow = EmailChallenge {
            flow_expires_at: T0 + Duration::minutes(2),
            ..challenge()
        };
        assert_eq!(
            decide_confirmation(&short_flow, &g, true, T0 + Duration::minutes(2)),
            ConfirmDecision::Reject(ChallengeRejection::Expired)
        );

        let mut attempts = challenge();
        assert_eq!(
            decide_confirmation(&attempts, &g, false, T0),
            ConfirmDecision::WrongCode {
                attempts: 1,
                invalidate: false
            }
        );
        attempts.attempts = 2;
        assert_eq!(
            decide_confirmation(&attempts, &g, false, T0),
            ConfirmDecision::WrongCode {
                attempts: 3,
                invalidate: true
            }
        );
        attempts.attempts = 3;
        assert_eq!(
            decide_confirmation(&attempts, &g, true, T0),
            ConfirmDecision::Reject(ChallengeRejection::AttemptsExhausted)
        );
        let done = confirmed();
        assert_eq!(
            decide_confirmation(&done, &g, true, T0),
            ConfirmDecision::Reject(ChallengeRejection::NotPending)
        );
    }

    #[test]
    fn proof_is_bound_to_flow_email_generation_and_consumed_once() {
        let window = Duration::seconds(60);
        let c = confirmed();
        let b = binding();
        let t = T0 + Duration::minutes(2);
        assert_eq!(
            decide_proof_use(Some(&c), &b, t, window),
            ProofDecision::Consume
        );
        assert_eq!(
            decide_proof_use(None, &b, t, window),
            ProofDecision::Reject(ProofRejection::Unknown)
        );
        assert_eq!(
            decide_proof_use(
                Some(&c),
                &ProofBinding {
                    flow_id: "flow-2".into(),
                    ..b.clone()
                },
                t,
                window
            ),
            ProofDecision::Reject(ProofRejection::FlowMismatch)
        );
        assert_eq!(
            decide_proof_use(
                Some(&c),
                &ProofBinding {
                    email_hash: "email-2".into(),
                    ..b.clone()
                },
                t,
                window
            ),
            ProofDecision::Reject(ProofRejection::EmailMismatch)
        );
        assert_eq!(
            decide_proof_use(
                Some(&c),
                &ProofBinding {
                    generation: generation("g2"),
                    ..b.clone()
                },
                t,
                window
            ),
            ProofDecision::Reject(ProofRejection::GenerationMismatch)
        );
        // 期限直前は有効、ちょうど・直後は期限切れ。
        assert_eq!(
            decide_proof_use(
                Some(&c),
                &b,
                c.proof_expires_at.unwrap() - Duration::seconds(1),
                window
            ),
            ProofDecision::Consume
        );
        for t in [
            c.proof_expires_at.unwrap(),
            c.proof_expires_at.unwrap() + Duration::seconds(1),
        ] {
            assert_eq!(
                decide_proof_use(Some(&c), &b, t, window),
                ProofDecision::Reject(ProofRejection::Expired)
            );
        }
    }

    #[test]
    fn consumed_proof_replays_only_inside_the_reuse_window_for_the_same_binding() {
        let window = Duration::seconds(60);
        let consumed_at = T0 + Duration::minutes(2);
        let c = EmailChallenge {
            consumed_at: Some(consumed_at),
            ..confirmed()
        };
        let b = binding();
        assert_eq!(
            decide_proof_use(Some(&c), &b, consumed_at + Duration::seconds(59), window),
            ProofDecision::Replay
        );
        for later in [
            Duration::seconds(60),
            Duration::seconds(61),
            Duration::minutes(5),
        ] {
            assert_eq!(
                decide_proof_use(Some(&c), &b, consumed_at + later, window),
                ProofDecision::Reject(ProofRejection::AlreadyConsumed)
            );
        }
        // 窓内でも別 flow / 別 email / 旧 generation の replay は拒否する。
        assert_eq!(
            decide_proof_use(
                Some(&c),
                &ProofBinding {
                    flow_id: "other".into(),
                    ..b.clone()
                },
                consumed_at + Duration::seconds(1),
                window
            ),
            ProofDecision::Reject(ProofRejection::FlowMismatch)
        );
        assert_eq!(
            decide_proof_use(
                Some(&c),
                &ProofBinding {
                    generation: generation("g0"),
                    ..b
                },
                consumed_at + Duration::seconds(1),
                window
            ),
            ProofDecision::Reject(ProofRejection::GenerationMismatch)
        );
    }

    #[test]
    fn canary_challenges_never_yield_a_registration_proof() {
        let canary = EmailChallenge {
            purpose: ChallengePurpose::Canary,
            ..confirmed()
        };
        assert_eq!(
            decide_proof_use(
                Some(&canary),
                &binding(),
                T0 + Duration::minutes(2),
                Duration::seconds(60)
            ),
            ProofDecision::Reject(ProofRejection::Unknown)
        );
    }
}
