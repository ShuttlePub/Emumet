//! Profile 配送 worker (P3o) の再試行方針と、送信結果から次の状態を決める純粋関数。
//!
//! 保存層 (`ProfileDeliveryJobRepository::record_attempt`) は結果を記録するだけで、バックオフと
//! 再試行上限の判断は worker の責務 (P2 確定契約)。ここがその判断の唯一の実装で、DB にも HTTP にも
//! 依存しない。

use super::{DeliveryAttemptOutcome, DeliveryFailureReason, DeliveryResultClass};
use time::{Duration, OffsetDateTime};

/// 配送 worker の方針。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileDeliveryPolicy {
    /// 1 つの job に対する最大試行回数 (今回の試行を含む。lease 期限切れで回収された試行も数える)。
    pub max_attempts: i32,
    /// 1 回目の失敗後のバックオフ。以降 2 倍ずつ増える。
    pub base_delay: Duration,
    pub max_delay: Duration,
    /// `Retry-After` を尊重する上限。
    pub retry_after_cap: Duration,
    /// 送信中の lease の長さ。送信側の名前解決・接続・全体 timeout の合計より長くする。
    pub lease_duration: Duration,
    /// 1 回の配送サイクルで処理する最大件数 (lease は 1 件ずつ取る)。
    pub batch_size: i64,
}

impl Default for ProfileDeliveryPolicy {
    /// 1 分から始めて 2 倍ずつ (上限 6 時間)、最大 12 回。初回から最後の再試行までおよそ 1 日。
    fn default() -> Self {
        Self {
            max_attempts: 12,
            base_delay: Duration::seconds(60),
            max_delay: Duration::hours(6),
            retry_after_cap: Duration::hours(6),
            lease_duration: Duration::seconds(120),
            batch_size: 10,
        }
    }
}

/// 送信側が観測した 1 回の結果 (分類前)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileSendObservation {
    /// 応答を受信した。`retry_after` は 429 / 503 の `Retry-After` を解釈した値。
    Response {
        status: u16,
        retry_after: Option<Duration>,
    },
    Timeout,
    ConnectFailed,
    DnsFailed,
    /// 宛先が不正、または内部宛先などで許可されない (送信していない)。
    InvalidTarget,
    /// 署名を作れなかった (送信していない)。鍵が後から用意される可能性があるので再試行する。
    SigningFailed,
}

#[derive(Clone, Copy)]
enum Classification {
    Success,
    Transient,
    Permanent,
}

fn classify(observation: &ProfileSendObservation) -> (Classification, DeliveryResultClass) {
    use Classification::*;
    use DeliveryResultClass as C;
    match observation {
        ProfileSendObservation::Response { status, .. } => match *status {
            200..=299 => (Success, C::Delivered),
            300..=399 => (Permanent, C::HttpRedirect),
            408 => (Transient, C::HttpRequestTimeout),
            429 => (Transient, C::HttpTooManyRequests),
            500..=599 => (Transient, C::HttpServerError),
            400..=499 => (Permanent, C::HttpClientError),
            _ => (Permanent, C::HttpUnexpectedStatus),
        },
        ProfileSendObservation::Timeout => (Transient, C::Timeout),
        ProfileSendObservation::ConnectFailed => (Transient, C::ConnectFailed),
        ProfileSendObservation::DnsFailed => (Transient, C::DnsFailed),
        ProfileSendObservation::InvalidTarget => (Permanent, C::InvalidTarget),
        ProfileSendObservation::SigningFailed => (Transient, C::SigningFailed),
    }
}

fn observed_status(observation: &ProfileSendObservation) -> Option<i32> {
    match observation {
        ProfileSendObservation::Response { status, .. } => Some(i32::from(*status)),
        _ => None,
    }
}

/// `attempt_no` (1 始まり。今回の試行を含む) 回目の失敗後のバックオフ。
/// `base_delay * 2^(n-1)` を `max_delay` で頭打ちにし、`[50%, 100%]` の範囲で jitter をかける。
/// `jitter` は `[0, 1)` の一様乱数 (範囲外・非数は丸める)。
pub fn profile_backoff_delay(
    attempt_no: i32,
    policy: &ProfileDeliveryPolicy,
    jitter: f64,
) -> Duration {
    let exponent = u32::try_from(attempt_no.saturating_sub(1).max(0))
        .unwrap_or(0)
        .min(30);
    let to_ms = |duration: Duration| {
        i64::try_from(duration.whole_milliseconds().max(0)).unwrap_or(i64::MAX)
    };
    let raw_ms = to_ms(policy.base_delay)
        .saturating_mul(1i64 << exponent)
        .min(to_ms(policy.max_delay));
    let jitter = if jitter.is_finite() {
        jitter.clamp(0.0, 1.0)
    } else {
        0.0
    };
    Duration::milliseconds((raw_ms as f64 * (0.5 + 0.5 * jitter)) as i64)
}

/// 送信結果から job の次の状態を決める。
///
/// - 2xx: `Delivered`
/// - 408 / 429 / 5xx / timeout / 接続失敗 / DNS 失敗 / 署名失敗: 再試行 (指数バックオフ + jitter。
///   429 と 503 の `Retry-After` は `retry_after_cap` を上限に尊重)。`max_attempts` に達したら
///   `Failed(RetriesExhausted)`
/// - 3xx・その他の 4xx・不正な宛先: `Failed(PermanentError)`
pub fn decide_profile_delivery(
    observation: &ProfileSendObservation,
    attempt_no: i32,
    now: OffsetDateTime,
    policy: &ProfileDeliveryPolicy,
    jitter: f64,
) -> DeliveryAttemptOutcome {
    let (classification, result) = classify(observation);
    let http_status = observed_status(observation);
    match classification {
        Classification::Success => DeliveryAttemptOutcome::Delivered { http_status },
        Classification::Permanent => DeliveryAttemptOutcome::Failed {
            reason: DeliveryFailureReason::PermanentError,
            result,
            http_status,
        },
        Classification::Transient => {
            if attempt_no >= policy.max_attempts {
                return DeliveryAttemptOutcome::Failed {
                    reason: DeliveryFailureReason::RetriesExhausted,
                    result,
                    http_status,
                };
            }
            let mut delay = profile_backoff_delay(attempt_no, policy, jitter);
            if let ProfileSendObservation::Response {
                status: 429 | 503,
                retry_after: Some(retry_after),
            } = observation
            {
                let honored = (*retry_after).clamp(Duration::ZERO, policy.retry_after_cap);
                delay = delay.max(honored);
            }
            DeliveryAttemptOutcome::Retry {
                next_attempt_at: now + delay,
                result,
                http_status,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH + Duration::days(20_000)
    }

    fn response(status: u16) -> ProfileSendObservation {
        ProfileSendObservation::Response {
            status,
            retry_after: None,
        }
    }

    #[test]
    fn success_statuses_are_delivered() {
        let policy = ProfileDeliveryPolicy::default();
        for status in [200, 201, 202, 204] {
            assert_eq!(
                decide_profile_delivery(&response(status), 1, now(), &policy, 0.5),
                DeliveryAttemptOutcome::Delivered {
                    http_status: Some(i32::from(status))
                }
            );
        }
    }

    #[test]
    fn transient_failures_retry_with_exponential_backoff() {
        let policy = ProfileDeliveryPolicy::default();
        for observation in [
            response(500),
            response(502),
            response(408),
            response(429),
            ProfileSendObservation::Timeout,
            ProfileSendObservation::ConnectFailed,
            ProfileSendObservation::DnsFailed,
            ProfileSendObservation::SigningFailed,
        ] {
            let outcome = decide_profile_delivery(&observation, 1, now(), &policy, 1.0);
            let DeliveryAttemptOutcome::Retry {
                next_attempt_at, ..
            } = outcome
            else {
                panic!("{observation:?} must retry, got {outcome:?}");
            };
            assert_eq!(next_attempt_at, now() + Duration::seconds(60));
        }
        let third = decide_profile_delivery(&response(500), 3, now(), &policy, 1.0);
        assert!(matches!(
            third,
            DeliveryAttemptOutcome::Retry { next_attempt_at, .. }
                if next_attempt_at == now() + Duration::seconds(240)
        ));
    }

    #[test]
    fn backoff_is_capped_and_jittered_between_half_and_full() {
        let policy = ProfileDeliveryPolicy::default();
        assert_eq!(
            profile_backoff_delay(1, &policy, 0.0),
            Duration::seconds(30)
        );
        assert_eq!(
            profile_backoff_delay(1, &policy, 1.0),
            Duration::seconds(60)
        );
        assert_eq!(
            profile_backoff_delay(40, &policy, 1.0),
            policy.max_delay,
            "the delay never exceeds max_delay"
        );
        assert_eq!(
            profile_backoff_delay(2, &policy, f64::NAN),
            Duration::seconds(60)
        );
    }

    #[test]
    fn permanent_failures_do_not_retry() {
        let policy = ProfileDeliveryPolicy::default();
        for (observation, result) in [
            (response(301), DeliveryResultClass::HttpRedirect),
            (response(400), DeliveryResultClass::HttpClientError),
            (response(401), DeliveryResultClass::HttpClientError),
            (response(410), DeliveryResultClass::HttpClientError),
            (
                ProfileSendObservation::InvalidTarget,
                DeliveryResultClass::InvalidTarget,
            ),
        ] {
            let outcome = decide_profile_delivery(&observation, 1, now(), &policy, 0.5);
            assert!(
                matches!(
                    outcome,
                    DeliveryAttemptOutcome::Failed {
                        reason: DeliveryFailureReason::PermanentError,
                        result: r,
                        ..
                    } if r == result
                ),
                "{observation:?} -> {outcome:?}"
            );
        }
    }

    #[test]
    fn retries_stop_at_max_attempts() {
        let policy = ProfileDeliveryPolicy {
            max_attempts: 3,
            ..ProfileDeliveryPolicy::default()
        };
        assert!(matches!(
            decide_profile_delivery(&response(503), 2, now(), &policy, 0.5),
            DeliveryAttemptOutcome::Retry { .. }
        ));
        assert!(matches!(
            decide_profile_delivery(&response(503), 3, now(), &policy, 0.5),
            DeliveryAttemptOutcome::Failed {
                reason: DeliveryFailureReason::RetriesExhausted,
                result: DeliveryResultClass::HttpServerError,
                http_status: Some(503),
            }
        ));
    }

    #[test]
    fn retry_after_is_honored_up_to_the_cap() {
        let policy = ProfileDeliveryPolicy::default();
        let honored = ProfileSendObservation::Response {
            status: 429,
            retry_after: Some(Duration::minutes(30)),
        };
        assert!(matches!(
            decide_profile_delivery(&honored, 1, now(), &policy, 0.0),
            DeliveryAttemptOutcome::Retry { next_attempt_at, .. }
                if next_attempt_at == now() + Duration::minutes(30)
        ));
        let capped = ProfileSendObservation::Response {
            status: 503,
            retry_after: Some(Duration::days(30)),
        };
        assert!(matches!(
            decide_profile_delivery(&capped, 1, now(), &policy, 0.0),
            DeliveryAttemptOutcome::Retry { next_attempt_at, .. }
                if next_attempt_at == now() + policy.retry_after_cap
        ));
        let ignored = ProfileSendObservation::Response {
            status: 500,
            retry_after: Some(Duration::days(1)),
        };
        assert!(matches!(
            decide_profile_delivery(&ignored, 1, now(), &policy, 1.0),
            DeliveryAttemptOutcome::Retry { next_attempt_at, .. }
                if next_attempt_at == now() + Duration::seconds(60)
        ));
    }
}
