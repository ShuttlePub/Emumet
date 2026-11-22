//! In-process counters of refused Relay admissions (429/503 of the Profile inbox), by a fixed,
//! non-confidential reason code. Nothing is written to the database per refusal (a refusal flood
//! must not add load, that would defeat the rate limit); the cumulative counts are logged by the
//! relay runtime's periodic telemetry tick (`post relay inbox refusals`) and a deployment alarm
//! watches them rise (docs/post-relay-operations.md). Reasons carry no body, actor or identifier.
use kernel::interfaces::post_relay::RefusalReason;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
static STORAGE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn record(reason: RefusalReason) {
    COUNTS[reason as usize].fetch_add(1, Ordering::Relaxed);
}

/// A 503 caused by a storage failure (not a quota or capacity decision).
pub(crate) fn record_storage() {
    STORAGE.fetch_add(1, Ordering::Relaxed);
}

fn count(reason: RefusalReason) -> u64 {
    COUNTS[reason as usize].load(Ordering::Relaxed)
}

/// Cumulative since process start; the field names are the fixed reason codes.
pub(crate) fn log_snapshot() {
    tracing::info!(
        global_rate = count(RefusalReason::GlobalRate),
        profile_rate = count(RefusalReason::ProfileRate),
        profile_quota = count(RefusalReason::ProfileQuota),
        global_quota = count(RefusalReason::GlobalQuota),
        quarantine_full = count(RefusalReason::QuarantineFull),
        capacity = count(RefusalReason::Capacity),
        storage = STORAGE.load(Ordering::Relaxed),
        "post relay inbox refusals"
    );
}
