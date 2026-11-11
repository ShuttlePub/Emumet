//! In-process sliding-window rate limiter for the username availability check.
//!
//! Emumet has no request-level rate limiting, so this is the smallest thing that protects the
//! endpoint from being used to enumerate usernames: one window per caller (`iss` + `sub`), kept
//! in memory of this instance only. Instances do not share state, so the effective limit of a
//! deployment with N instances is up to N times the configured one.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub(crate) const DEFAULT_MAX_REQUESTS: usize = 60;
pub(crate) const DEFAULT_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RateLimitDecision {
    Allowed,
    Limited { retry_after: Duration },
}

#[derive(Debug)]
struct State {
    requests: HashMap<(String, String), VecDeque<Instant>>,
    last_sweep: Option<Instant>,
}

#[derive(Debug)]
pub(crate) struct UsernameAvailabilityRateLimiter {
    max_requests: usize,
    window: Duration,
    state: Mutex<State>,
}

impl Default for UsernameAvailabilityRateLimiter {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_REQUESTS, DEFAULT_WINDOW)
    }
}

impl UsernameAvailabilityRateLimiter {
    pub(crate) fn new(max_requests: usize, window: Duration) -> Self {
        Self {
            max_requests,
            window,
            state: Mutex::new(State {
                requests: HashMap::new(),
                last_sweep: None,
            }),
        }
    }

    pub(crate) fn check(&self, issuer: &str, subject: &str) -> RateLimitDecision {
        self.check_at(issuer, subject, Instant::now())
    }

    /// `now` is injected so that the window can be exercised deterministically. A rejected
    /// request is not recorded, so being limited does not extend the limit.
    pub(crate) fn check_at(&self, issuer: &str, subject: &str, now: Instant) -> RateLimitDecision {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.sweep_if_due(&mut state, now);

        let timestamps = state
            .requests
            .entry((issuer.to_string(), subject.to_string()))
            .or_default();
        while timestamps
            .front()
            .is_some_and(|oldest| now.saturating_duration_since(*oldest) >= self.window)
        {
            timestamps.pop_front();
        }
        if timestamps.len() >= self.max_requests {
            let retry_after = timestamps
                .front()
                .map(|oldest| {
                    self.window
                        .saturating_sub(now.saturating_duration_since(*oldest))
                })
                .unwrap_or(self.window);
            return RateLimitDecision::Limited { retry_after };
        }
        timestamps.push_back(now);
        RateLimitDecision::Allowed
    }

    /// Drops callers whose whole window has expired, at most once per window, so memory is
    /// bounded by the callers seen within roughly the last two windows.
    fn sweep_if_due(&self, state: &mut State, now: Instant) {
        let due = state
            .last_sweep
            .is_none_or(|last| now.saturating_duration_since(last) >= self.window);
        if !due {
            return;
        }
        state.last_sweep = Some(now);
        let window = self.window;
        state.requests.retain(|_, timestamps| {
            timestamps
                .back()
                .is_some_and(|newest| now.saturating_duration_since(*newest) < window)
        });
    }

    #[cfg(test)]
    fn tracked_callers(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .requests
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter() -> UsernameAvailabilityRateLimiter {
        UsernameAvailabilityRateLimiter::new(3, Duration::from_secs(10))
    }

    #[test]
    fn allows_up_to_the_limit_then_limits_with_retry_after() {
        let limiter = limiter();
        let start = Instant::now();
        for i in 0..3 {
            assert_eq!(
                limiter.check_at("iss", "alice", start + Duration::from_secs(i)),
                RateLimitDecision::Allowed
            );
        }
        assert_eq!(
            limiter.check_at("iss", "alice", start + Duration::from_secs(4)),
            RateLimitDecision::Limited {
                retry_after: Duration::from_secs(6)
            }
        );
    }

    #[test]
    fn callers_are_limited_independently() {
        let limiter = limiter();
        let now = Instant::now();
        for _ in 0..3 {
            assert_eq!(
                limiter.check_at("iss", "alice", now),
                RateLimitDecision::Allowed
            );
        }
        assert!(matches!(
            limiter.check_at("iss", "alice", now),
            RateLimitDecision::Limited { .. }
        ));
        assert_eq!(
            limiter.check_at("iss", "bob", now),
            RateLimitDecision::Allowed
        );
        assert_eq!(
            limiter.check_at("other-iss", "alice", now),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn recovers_when_the_window_has_passed() {
        let limiter = limiter();
        let start = Instant::now();
        for _ in 0..3 {
            limiter.check_at("iss", "alice", start);
        }
        assert!(matches!(
            limiter.check_at("iss", "alice", start + Duration::from_secs(9)),
            RateLimitDecision::Limited { .. }
        ));
        assert_eq!(
            limiter.check_at("iss", "alice", start + Duration::from_secs(10)),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn limited_requests_do_not_count_towards_the_window() {
        let limiter = limiter();
        let start = Instant::now();
        for _ in 0..3 {
            limiter.check_at("iss", "alice", start);
        }
        for second in 1..10 {
            assert!(matches!(
                limiter.check_at("iss", "alice", start + Duration::from_secs(second)),
                RateLimitDecision::Limited { .. }
            ));
        }
        assert_eq!(
            limiter.check_at("iss", "alice", start + Duration::from_secs(10)),
            RateLimitDecision::Allowed,
            "the rejected requests must not have been recorded"
        );
    }

    #[test]
    fn expired_callers_are_swept_so_memory_stays_bounded() {
        let limiter = limiter();
        let start = Instant::now();
        for i in 0..100 {
            limiter.check_at("iss", &format!("user-{i}"), start);
        }
        assert_eq!(limiter.tracked_callers(), 100);
        limiter.check_at("iss", "latecomer", start + Duration::from_secs(25));
        assert_eq!(limiter.tracked_callers(), 1);
    }
}
