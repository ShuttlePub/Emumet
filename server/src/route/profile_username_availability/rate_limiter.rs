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
    /// Latest time ever observed. Time never goes backwards for the window logic, so each
    /// caller's timestamps stay non-decreasing even if callers read the clock out of order.
    latest: Option<Instant>,
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
                latest: None,
            }),
        }
    }

    pub(crate) fn check(&self, issuer: &str, subject: &str) -> RateLimitDecision {
        // The clock is read while holding the lock: reading it first would let a thread that
        // read an earlier time acquire the lock after one that read a later time.
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.decide(&mut state, issuer, subject, Instant::now())
    }

    /// `now` is injected so that the window can be exercised deterministically. A rejected
    /// request is not recorded, so being limited does not extend the limit.
    #[cfg(test)]
    pub(crate) fn check_at(&self, issuer: &str, subject: &str, now: Instant) -> RateLimitDecision {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.decide(&mut state, issuer, subject, now)
    }

    fn decide(
        &self,
        state: &mut State,
        issuer: &str,
        subject: &str,
        observed: Instant,
    ) -> RateLimitDecision {
        let now = state.latest.map_or(observed, |latest| latest.max(observed));
        state.latest = Some(now);
        self.sweep_if_due(state, now);

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
    fn timestamps_are_non_decreasing(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.requests.values().all(|timestamps| {
            timestamps
                .iter()
                .zip(timestamps.iter().skip(1))
                .all(|(a, b)| a <= b)
        })
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
    fn out_of_order_clock_reads_cannot_break_the_window_or_the_sweep() {
        let limiter = limiter();
        let start = Instant::now();
        let later = start + Duration::from_secs(20);
        assert_eq!(
            limiter.check_at("iss", "alice", later),
            RateLimitDecision::Allowed
        );
        // A thread that read the clock earlier but reached the lock afterwards.
        assert_eq!(
            limiter.check_at("iss", "alice", start),
            RateLimitDecision::Allowed
        );
        assert!(limiter.timestamps_are_non_decreasing());
        assert_eq!(
            limiter.check_at("iss", "alice", later),
            RateLimitDecision::Allowed
        );
        assert!(matches!(
            limiter.check_at("iss", "alice", later),
            RateLimitDecision::Limited { .. }
        ));
        // The sweep must still see alice as active (its newest entry is the latest one).
        limiter.check_at("iss", "bob", later + Duration::from_secs(9));
        assert!(matches!(
            limiter.check_at("iss", "alice", later + Duration::from_secs(9)),
            RateLimitDecision::Limited { .. }
        ));
        assert!(limiter.timestamps_are_non_decreasing());
    }

    #[test]
    fn concurrent_checks_keep_timestamps_ordered_and_the_limit_exact() {
        let limiter = std::sync::Arc::new(UsernameAvailabilityRateLimiter::new(
            200,
            Duration::from_secs(60),
        ));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let limiter = limiter.clone();
                std::thread::spawn(move || {
                    (0..50)
                        .filter(|_| limiter.check("iss", "alice") == RateLimitDecision::Allowed)
                        .count()
                })
            })
            .collect();
        let allowed: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert_eq!(allowed, 200);
        assert!(limiter.timestamps_are_non_decreasing());
        assert!(matches!(
            limiter.check("iss", "alice"),
            RateLimitDecision::Limited { .. }
        ));
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
