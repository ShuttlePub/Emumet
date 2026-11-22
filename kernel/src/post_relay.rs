//! Transaction-participating P1 ports. No sender, inbox activation or management HTTP API.
use crate::database::{DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::ProfileId;
use crate::KernelError;
use std::future::Future;
use time::{Duration, OffsetDateTime};

/// Signed immutable bytes. Transport must include DNS/connect/TLS/response in deadline.
pub struct RelayHttpRequest<'a> {
    pub origin: &'a str,
    pub envelope: &'a [u8],
    pub headers: &'a [(String, String)],
    pub deadline: std::time::Instant,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayHttpResult {
    Response {
        status: u16,
        retry_after_secs: Option<u64>,
    },
    InvalidTarget,
    Transient,
}
pub trait PostRelayTransport: Send + Sync + 'static {
    fn origin_allowed(&self, origin: &str) -> bool;
    fn send(&self, request: &RelayHttpRequest<'_>) -> impl Future<Output = RelayHttpResult> + Send;
}
pub trait DependOnPostRelayTransport: Send + Sync {
    type PostRelayTransport: PostRelayTransport;
    fn post_relay_transport(&self) -> &Self::PostRelayTransport;
}

pub const AUTOMATIC_TTL: Duration = Duration::hours(72);
pub const QUARANTINE_TTL: Duration = Duration::days(30);
pub const SEND_TIMEOUT: Duration = Duration::seconds(10);
pub const LEASE_TTL: Duration = Duration::seconds(30);
pub const ATTEMPT_ROWS: i64 = 16;
pub const RESERVATION_ROWS: i64 = 17;
pub const RESERVATION_BYTES: i64 = 16 * 512 + 1024;
pub const BODY_METADATA_BYTES: i64 = 4096;
pub const MAX_BATCH: u16 = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayRecipient {
    pub link_id: String,
    pub generation: i64,
    pub origin: String,
}
/// Final v1 envelope. Deliberately no Debug/Serialize: never log payloads.
pub struct RelayAdmission {
    pub profile_id: ProfileId,
    pub receipt: String,
    pub received_at: OffsetDateTime,
    pub recipient: Option<RelayRecipient>,
    pub envelope: Vec<u8>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitScope {
    Profile,
    Global,
}
/// Fixed, non-confidential reason of a refused admission (counted, never carries identifiers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalReason {
    GlobalRate,
    ProfileRate,
    ProfileQuota,
    GlobalQuota,
    QuarantineFull,
    Capacity,
}
impl RefusalReason {
    pub const ALL: [RefusalReason; 6] = [
        RefusalReason::GlobalRate,
        RefusalReason::ProfileRate,
        RefusalReason::ProfileQuota,
        RefusalReason::GlobalQuota,
        RefusalReason::QuarantineFull,
        RefusalReason::Capacity,
    ];
    pub const fn code(self) -> &'static str {
        match self {
            RefusalReason::GlobalRate => "global_rate",
            RefusalReason::ProfileRate => "profile_rate",
            RefusalReason::ProfileQuota => "profile_quota",
            RefusalReason::GlobalQuota => "global_quota",
            RefusalReason::QuarantineFull => "quarantine_full",
            RefusalReason::Capacity => "capacity",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    Queued,
    Unlinked,
    Limited {
        scope: LimitScope,
        retry_after_secs: u32,
        reason: RefusalReason,
    },
    Unavailable,
}
#[derive(Clone, Debug)]
pub struct RelayLease {
    pub receipt: String,
    pub profile_id: ProfileId,
    pub token: i64,
    pub lease_until: OffsetDateTime,
    /// Ready queue latency (due_at -> claim DB time), individually sampled, including preload.
    /// Intentional retry/backoff before due_at is excluded. No per-success DB row is retained.
    pub queue_wait_ns: u64,
}
/// Lock-free preparation snapshot. NOT permission: release its DB connection before keys, DNS
/// and signing; authorize_send must recheck after preparation. Never log envelope bytes.
pub struct RelayPreparation {
    pub recipient: RelayRecipient,
    pub envelope: Vec<u8>,
    pub expires_at: OffsetDateTime,
    pub manual: bool,
}
/// NOT permission until the containing transaction commits successfully. Never reuse after
/// restart or unknown commit. No database connection survives this value.
pub struct SendPermit {
    pub lease: RelayLease,
    pub recipient: RelayRecipient,
    pub envelope: Vec<u8>,
    pub attempt_id: i64,
    pub authorized_at: OffsetDateTime,
    pub send_deadline: OffsetDateTime,
    pub manual: bool,
}
pub enum Authorization {
    Permitted(SendPermit),
    Held,
    Expired,
    Stale,
    Busy,
}
/// A trusted S1 observation on the authorization DB timeline, within the committed permit.
/// S1 MUST use its monotonic send deadline, never label an after-deadline response in time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttemptObservation {
    pub attempt_id: i64,
    pub observed_at: OffsetDateTime,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptResult {
    Success,
    Retry {
        status: Option<u16>,
        retry_after_secs: Option<u64>,
    },
    Permanent {
        status: Option<u16>,
    },
}
impl AttemptResult {
    pub fn http(status: u16, retry_after_secs: Option<u64>) -> Self {
        match status {
            200..=299 => Self::Success,
            401 | 408 | 429 | 500..=599 => Self::Retry {
                status: Some(status),
                retry_after_secs,
            },
            _ => Self::Permanent {
                status: Some(status),
            },
        }
    }
}
/// Equal jitter inclusive 50..100%; Retry-After is a floor clamped to 1h and remaining TTL.
/// Randomness is supplied by the caller (production CSPRNG, deterministic in tests).
pub fn retry_delay(
    failures: u64,
    random: u64,
    retry_after_secs: Option<u64>,
    remaining: Duration,
) -> Duration {
    let base = 30u64
        .saturating_mul(
            1u64.checked_shl(failures.saturating_sub(1).min(63) as u32)
                .unwrap_or(u64::MAX),
        )
        .min(3600);
    let jitter_ms = base * 500 + random % (base * 500 + 1);
    Duration::milliseconds(jitter_ms.max(retry_after_secs.unwrap_or(0).min(3600) * 1000) as i64)
        .min(remaining.max(Duration::ZERO))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recorded {
    Applied,
    Stale,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperatorAction {
    Redeliver,
    Discard,
}
#[derive(Clone)]
pub struct OperatorRequest {
    pub id: String,
    pub profile_id: ProfileId,
    pub receipt: Option<String>,
    pub action: OperatorAction,
    pub operator: String,
    pub reason: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorPreview {
    pub count: i64,
    pub bytes: i64,
    pub boundary: i64,
    pub earliest_expiry: Option<OffsetDateTime>,
}
#[derive(Clone, Debug)]
pub struct OperatorBatch {
    pub selected: u16,
    pub changed: u16,
    pub complete: bool,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct CapacityReport {
    pub free_bytes: i64,
    pub total_bytes: i64,
    pub sampled_at: OffsetDateTime,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct RelayCapacity {
    pub succeeded: i64,
    pub cancelled: i64,
    pub permanent: i64,
    pub expired: i64,
    pub retries: i64,
    pub held: i64,
    pub purged: i64,
    pub body_count: i64,
    pub body_bytes: i64,
    pub delivery_rows: i64,
    pub delivery_bytes: i64,
    pub unlinked_rows: i64,
    pub operator_rows: i64,
    pub consistent: bool,
    pub physical_accepting: bool,
    pub sampled_at: Option<OffsetDateTime>,
    pub oldest_received_at: Option<OffsetDateTime>,
    pub earliest_automatic_expiry: Option<OffsetDateTime>,
    pub earliest_quarantine_expiry: Option<OffsetDateTime>,
    pub last_purge_at: Option<OffsetDateTime>,
    pub cancelled_body_backlog: i64,
}

/// Mutations REQUIRE an explicit transaction: join it, never commit. Order:
/// Account/Profile/link -> global/Profile quota -> slots -> job/attempt/audit.
/// I1 calls prepare_admission before relationship locks; enqueue joins receipt/Undo writes.
/// Claim-only/discovery use a separate short transaction, never climb from job to gate.
/// Batches settle one Profile per transaction. Changed owner => rollback/retry Concurrency.
/// On errors/limits roll back (429 Profile, 503 Global/Unavailable). Only commit permits sending.
pub trait PostRelayStore: Send + Sync + 'static {
    type Connection: crate::database::Connection;
    /// Database clock for I1 to freeze receivedAt/envelope before enqueue (avoid host clock skew).
    fn admission_time(
        &self,
        c: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<OffsetDateTime, KernelError>> + Send;
    fn prepare_admission(
        &self,
        c: &mut Self::Connection,
        profile: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<Option<RelayRecipient>, KernelError>> + Send;
    fn enqueue(
        &self,
        c: &mut Self::Connection,
        request: &RelayAdmission,
    ) -> impl Future<Output = error_stack::Result<Admission, KernelError>> + Send;
    fn claim_due(
        &self,
        c: &mut Self::Connection,
        limit: u16,
    ) -> impl Future<Output = error_stack::Result<Vec<RelayLease>, KernelError>> + Send;
    /// S1 fair scheduling. Leaves the low-level Q1 claim port unchanged for recovery/testing.
    fn claim_scheduled(
        &self,
        c: &mut Self::Connection,
        limit: u16,
        operation: Option<&str>,
    ) -> impl Future<Output = error_stack::Result<Vec<RelayLease>, KernelError>> + Send;
    /// A separate short read transaction, no lifecycle/job locks and no new permit.
    fn prepare_send(
        &self,
        c: &mut Self::Connection,
        lease: &RelayLease,
    ) -> impl Future<Output = error_stack::Result<Option<RelayPreparation>, KernelError>> + Send;
    fn renew_lease(
        &self,
        c: &mut Self::Connection,
        lease: &RelayLease,
    ) -> impl Future<Output = error_stack::Result<Recorded, KernelError>> + Send;
    fn authorize_send(
        &self,
        c: &mut Self::Connection,
        lease: &RelayLease,
        origin_allowed: bool,
    ) -> impl Future<Output = error_stack::Result<Authorization, KernelError>> + Send;
    /// Completed HTTP results require the permit attempt and a trusted in-deadline observation.
    /// None is only pre-permission failure / transport timeout; a late response is not in time.
    /// A live fenced in-time success/permanent result wins over delayed DB save past expiry.
    fn record_attempt(
        &self,
        c: &mut Self::Connection,
        lease: &RelayLease,
        result: AttemptResult,
        observation: Option<&AttemptObservation>,
        random: u64,
    ) -> impl Future<Output = error_stack::Result<Recorded, KernelError>> + Send;
    fn finish_success(
        &self,
        c: &mut Self::Connection,
        lease: &RelayLease,
        observation: &AttemptObservation,
    ) -> impl Future<Output = error_stack::Result<Recorded, KernelError>> + Send {
        self.record_attempt(c, lease, AttemptResult::Success, Some(observation), 0)
    }
    fn hold(
        &self,
        c: &mut Self::Connection,
        lease: &RelayLease,
    ) -> impl Future<Output = error_stack::Result<Recorded, KernelError>> + Send;
    fn maintenance_due(
        &self,
        c: &mut Self::Connection,
        limit: u16,
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileId>, KernelError>> + Send;
    fn expire_due(
        &self,
        c: &mut Self::Connection,
        profile: &ProfileId,
        limit: u16,
    ) -> impl Future<Output = error_stack::Result<u64, KernelError>> + Send;
    fn purge_expired(
        &self,
        c: &mut Self::Connection,
        profile: &ProfileId,
        limit: u16,
    ) -> impl Future<Output = error_stack::Result<u64, KernelError>> + Send;
    fn operator_preview(
        &self,
        c: &mut Self::Connection,
        request: &OperatorRequest,
    ) -> impl Future<Output = error_stack::Result<OperatorPreview, KernelError>> + Send;
    fn begin_operator(
        &self,
        c: &mut Self::Connection,
        request: &OperatorRequest,
    ) -> impl Future<Output = error_stack::Result<OperatorPreview, KernelError>> + Send;
    fn operator_redeliver(
        &self,
        c: &mut Self::Connection,
        operation: &str,
    ) -> impl Future<Output = error_stack::Result<OperatorBatch, KernelError>> + Send;
    fn operator_discard(
        &self,
        c: &mut Self::Connection,
        operation: &str,
        confirmed: bool,
    ) -> impl Future<Output = error_stack::Result<OperatorBatch, KernelError>> + Send;
    fn report_capacity(
        &self,
        c: &mut Self::Connection,
        report: &CapacityReport,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
    fn read_capacity(
        &self,
        c: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<RelayCapacity, KernelError>> + Send;
    fn check_reservations(
        &self,
        c: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;
}
pub trait DependOnPostRelayStore: DependOnDatabaseConnection {
    type PostRelayStore: PostRelayStore<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;
    fn post_relay_store(&self) -> &Self::PostRelayStore;
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_boundaries_and_http_classification() {
        assert_eq!(
            retry_delay(1, 0, None, Duration::hours(1)),
            Duration::seconds(15)
        );
        assert_eq!(
            retry_delay(1, 15000, None, Duration::hours(1)),
            Duration::seconds(30)
        );
        assert_eq!(
            retry_delay(100, 0, Some(u64::MAX), Duration::hours(2)),
            Duration::hours(1)
        );
        assert_eq!(
            retry_delay(1, 0, Some(300), Duration::seconds(2)),
            Duration::seconds(2)
        );
        assert_eq!(retry_delay(1, 0, None, Duration::ZERO), Duration::ZERO);
        for s in [401, 408, 429, 500, 503, 599] {
            assert!(matches!(
                AttemptResult::http(s, None),
                AttemptResult::Retry { .. }
            ));
        }
        for s in [300, 301, 400, 403, 409] {
            assert!(matches!(
                AttemptResult::http(s, None),
                AttemptResult::Permanent { .. }
            ));
        }
        assert_eq!(AttemptResult::http(204, None), AttemptResult::Success);
    }
}
