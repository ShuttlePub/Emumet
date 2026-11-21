# P1 post-relay storage (Q1)

Baseline: a625d59 (L1, PR #120). Additive migration 20261122000300. Canonical envelope
contract remains docs/contracts/, v1 revision 3; its snapshot is NOT independently edited.
Existing Profile delivery tables/policy are unchanged. No inbox route or sender is enabled.

## Transaction ports for I1 and S1

kernel::interfaces::post_relay::PostRelayStore is exposed through DependOnPostRelayStore,
PostgresDatabase and the database delegation macro. Application's AdmitPostRelayUseCase joins
the supplied connection; it never opens another transaction or commits a handler's receipt.

- I1: admission_time supplies the DB timestamp for receivedAt (do not use an independently skewed
  application clock), then prepare_admission (Account/Profile/relationship/link gates + current
  event-sourced owner/actor), then
  existing relationship/receipt/Undo work on the SAME connection, then enqueue. enqueue performs
  shared admission_rate_limit and reserve_capacity atomically with the job or unlinked bucket;
  it rechecks recipient/generation. Duplicate in an old inbound receipt table is not enqueue proof.
  The caller constructs final v1 bytes once using that recipient and a fresh opaque receipt.
- Roll back on any error or Admission::Limited/Unavailable. Return 202 only after commit.
  Profile limits map to 429, global limits/unknown physical capacity to 503, each Retry-After 1s;
  persistence errors are retryable 5xx. Never return an unaudited unlinked 202.
- S1: claim_due in a separate short transaction (job-only SKIP LOCKED; never climb to gates).
  prepare_send reads immutable bytes/recipient/expiry in a separate lock-free short transaction,
  fenced by the live lease and generation, before key/signature/DNS/policy preparation. It creates
  NO permission. Close that connection before external preparation.
  renew_lease, authorize_send, record_attempt, finish_success, hold use the common lifecycle order.
  authorize_send takes an origin policy decision already prepared OUTSIDE the transaction.
  Authorization::Permitted is only a proposed permit: a successful COMMIT is the start of sending.
  Unknown commit => NO send. No connection/transaction is carried by SendPermit.
- P4 (response obtained, DB recording delayed): record_attempt takes Option<&AttemptObservation>
  (permit attempt_id plus observed_at), and finish_success REQUIRES that observation. S1 attests
  that it obtained the result within the SAME monotonic send deadline; observed_at is correlated
  to the permit's DB authorization timeline, NOT an independently skewed application wall clock
  or a receiver Date. It must be within [authorized_at, send_deadline]. Never claim that a late
  response was in time. No observation is only for pre-permission preparation failures or a
  transport timeout without a completed response (Retry with no HTTP status after permission).
  The store rechecks the current lease/fencing/generation/owner and attempt ID, and resamples the
  lease clock AFTER job-row lock waits. A valid in-deadline 2xx deletes the body even if DB saving
  crosses 72h/the original manual quarantine expiry; a permanent result uses OBSERVED time as
  logical terminal, not late DB save time. A retry saved after the automatic cutoff still ends
  at that cutoff. Wrong attempt, revoked generation or lost lease is stale even for 2xx; out-of-
  permit observation/missing success observation is rejected without releasing slots/capacity.
  Expiry and quarantine body purge leave a live authorized lease time to record P4, only until
  lease loss; no new send/claim/observation window is granted, and no retention deadline resets.
- The final permission UPDATE samples clock_timestamp once. Its absolute deadline is the lesser
  of authorized_at +10s and received_at +72h (manual: original quarantine expiry). Commit latency
  consumes that deadline. S1 can conservatively anchor (send_deadline - authorized_at) to an
  Instant taken BEFORE the authorization transaction, subtracting all elapsed/commit time; never
  use a separately skewed application wall clock to extend it. Convert to a monotonic deadline,
  never restart its clock,
  never reuse after process restart, and perform HTTP with zero held DB connections/locks.
- Lease: 30s, globally non-reused sequence fencing token, independent attempt ID. A claim cannot
  reclaim before both lease and old permission deadline. Sixteen fixed DB permit slots, max four
  per origin, are shared by all processes/manual jobs and survive unlink until the old deadline.
  Stale results cannot release slots, capacity, bodies, or revive jobs. Results after lease loss
  are ignored. Slot exhaustion is Busy; S1 handles fair scheduling/backpressure, not a busy loop.
- maintenance_due discovers bounded Profile targets. For each target in a FRESH transaction,
  expire_due and purge_expired settle gates/link/quota before body/job work (maximum 128 bodies
  and 128 rows per expired metadata category in a purge transaction).
  S1/T1 must schedule these ports and monitor errors/delay; Q1 starts no background sender.
- read_capacity reports logical rows/bytes, fixed counters, oldest/earliest deadlines, physical
  sample age, purge watermark and cancelled_body_backlog. check_reservations compares actual
  body/job/audit/bucket/operation usage against global/Profile/link reservations under the global
  quota lock; mismatch emits a FIXED error and stops admission until a successful reconciliation.
  It does NOT repair or silently drop accepted content.

Lock order remains Account -> Profile (IDs ascending) -> existing relations -> link (IDs ascending)
-> global quota -> Profile quota -> shared slots -> bodies/jobs/quarantine -> attempts/audits.
Changed ownership requires rollback and retry from discovery, not an additional lower gate.
Only gate-bearing transactions use L1's 2s lock /3s statement /5s wall-clock+idle bounds.
I1/S1 must use a dedicated short-DB pool (initial max 4 connections/process) rather than retain
API connections during HTTP. Throughput/fairness and HTTP cancellation stop-point tests belong to S1/I1.

## Retention, immutable bytes and reserved space

post_relay_bodies owns immutable receipt, origin, link/generation, received_at and envelope bytes.
An UPDATE trigger rejects changes. Jobs/dead letters reference this ONE copy, not a second body
when quarantine/manual delivery is staged. No FK to disposable Profile/Account projections.
No body is put in an event stream. Logical body bytes = envelope length +4096 bounded metadata
(including the one-to-one job/quarantine references). The separate metadata row cap below covers
attempts, body-free audits, aggregates and their reservations, as specified in plan §6.4; it is
not a second independent job/quarantine body quota.

| Partition | Global | Profile |
| --- | --- | --- |
| common jobs + quarantine | 100,000 /20GiB | 2,000 /512MiB |
| delivery metadata incl reservations | 1,800,000 /1800MiB | 48,000 /60MiB |
| unlinked aggregates AND their Profile state | 100,000 rows x512 bytes | 1,000 rows x512 bytes |
| protected operation audits | 100,000 rows x1024 bytes | 1,000 rows x1024 bytes |

Metadata sums to <=2,000,000 rows /<2GiB globally and <=50,000 rows /<64MiB/Profile.
64 delivery rows /64KiB conservatively reserve fixed quota/counter/slot/capacity state. A linked
Profile state costs one delivery row/512 bytes; an unlinked-only state is charged to the UNLINKED
partition, never to delivery reservations. Linking converts that one header atomically. Therefore
exhausting aggregates (including their headers) cannot spend linked job reservations. Idle empty
Profile state is purged after 30 days, once all physically retained bodies are gone.

Each admitted automatic job reserves 17 metadata rows /9216 bytes (16 x512 attempts +1024 summary).
At permanent failure or 72h cut-off it converts this reservation to ONE 1024-byte audit row and a
30-day quarantine body, without increasing body use. All accepted jobs may fail at once: quarantine
cannot run out of its own independent budget because there is no independent body budget.
Actual persisted attempts are the last 16; total/first/last/status are bounded aggregate fields.

Success: delete body/job/attempts, release reservation, increment ONE fixed counter. No receipt
success row. Cancellation: logical release/fixed counter immediately; physical delete below.
Permanent/expired: delete job/attempts, keep one body/dead-letter and one body-free summary only
until **logical terminal time +30 days**, never DB collection/commit time +30 days. Logical terminal
time is received_at +72h for automatic expiry, or the permanent failure observation/attempt time
(no later than that 72h cutoff). Holds, lease recovery, restarts and a long worker outage extend
NEITHER deadline. Thirty days means 720 elapsed hours, including SQL constraints/cleanup (not
session-timezone calendar arithmetic across DST). Thus automatic-expiry retention ends at received_at +33 days; an earlier
permanent failure ends sooner. If first collection is AT or AFTER that fixed quarantine expiry,
erase the body in that transaction without creating a dead letter: a long outage can skip
quarantine entirely and manual redelivery is impossible. Still record the body-free failure
summary with that SAME original terminal/expiry (already purge-due), never a fresh 30-day audit
window. Bounded audit purge then removes it normally. Only these failures use the ADR 0003
exception. No early automatic eviction for new arrivals. Manual success deletes the body but retains the original failure audit
only to its original deadline. Purge never relies on caller-supplied remote IDs as keys/labels.
Unlinked: Profile + fixed no_home reason + UTC hour, count/first/last only; no receipt, header,
remote origin, Activity ID or body. Expiry is first aggregation +30d, never refreshed by new counts.

Backoff: min(30s x2^(failures-1),1h), equal jitter 50..100%. Retry-After is a floor capped at 1h
and remaining TTL. 401/408/429/5xx and transport/temporary signing errors retry; 3xx and other 4xx
are permanent. Tests inject time and random values; no wall-clock waiting for 72h/30d boundaries.

## Cancellation strategy and measurements

Synchronous physical deletion was considered first, as required by the packet. Initial offline
probe used 8 Profiles x2000 bodies (~512MiB each) with uncompressed external TOAST and 16 attempts
per body. One Profile deletion: 600.235ms. Seven more: statement timeout at 3000.367ms, rollback.
That initial probe used 1KiB metadata accounting (536,870,000 logical bytes/Profile); final storage
uses a conservative 4KiB header. The committed offline probe reproduces the same experiment at
EXACTLY 536,870,912 bytes/Profile with that final header. Use ONLY a disposable DB: it writes
~4GiB uncompressed body data plus WAL and attempts:

~~~sh
psql "$DISPOSABLE_DATABASE_URL" -X -f tools/measure-post-relay-cancellation.sql
~~~

The committed EXACT-limit reproducer was also run against its own disposable DB: all eight
Profiles were 2000 bodies / 536,870,912 logical bytes. Physical deletion of one Profile took
456.414ms; deletion of the remaining seven hit statement_timeout at 3000.221ms and rolled back.
The disposable DB was then dropped. Local timing samples use development PostgreSQL with
fsync=off and synchronous_commit=on, not production durability/performance settings.

As explicitly authorized in issue #121's packet, Q1 therefore uses the bounded physical-delete
fallback: L1's THREE revokers invalidate the link/generation and release aggregate link/Profile/
global logical reservations IN THE SAME transaction. Gate proof is required. Failure rolls back
link invalidation and release together. No new automatic/manual authorization can see the old
link, even if Profile rows survive or bodies await purge. Prior committed permission may complete
only within its original deadline. Never wait for HTTP. No restoration by stale worker results.

Physical body/job/attempt removal is later, at most 128 bodies per Profile transaction, including
quarantine. It is not post-commit queueing of a cancellable external side effect: persistent
unlinked ledger/released usage is itself the durable cleanup predicate, including after a crash.
Immediate logical erasure is NOT a claim of immediate disk/MVCC/backup erasure. Monitor backlog
and volume headroom; physical sampling continues to stop new admission if cleanup cannot keep up.

Final adapter full-limit test: eight Profiles, EXACT 512MiB/2000 bodies each (half jobs with 16
attempts, half quarantine+audit); compressible fixtures bound CI disk use. Bulk fixture generation
is offline setup, outside lifecycle transactions; it is not a timed production admission. Every
measured unlink/deactivation/purge still uses the unchanged 2s/3s/5s bounds. Local result (PG17.9,
debug, Unix socket, AMD Ryzen 9 7900, 24 logical CPUs/64GiB host, Linux 7.3.5): Profile unlink
1.997ms; Account deactivation of seven remaining Profiles 6.705ms; retained physical bodies after
commit 16,000; worst 128-body cleanup batch 16.915ms. These are measurements, not production SLOs.
Both uncompressed probes reject all-in-transaction physical deletion, not the compressible CI fixture.

Row-count test at 256 and 768 successful 64KiB relays: ZERO remaining body/job/attempt/dead-letter/
audit/link-usage rows at both checkpoints, ONE Profile quota and fixed 16 slots/counters. Success
volume does not grow rows. It is not a 10-minute 100 HTTP/s benchmark; S1/I1 own that measurement.
Full-failure test admits 2000 bodies, converts ALL to quarantine and summaries without reservation
failure, then rejects new arrivals rather than evicting old content. Concurrent byte test accepts
exactly 255 maximum-size envelopes before 512MiB. Two separate OS processes share a 200-token
Profile burst. Global bucket is 1000/s (burst1000), Profile200/s (burst200); unlinked has its own
1000/s global bucket so neither its rate traffic nor its aggregates spend linked global admission.
Q1 fixes these initial values; I1 must measure contention and obtain parent approval for any change.

## Operator storage frames (S1 commands are NOT implemented here)

operator_preview -> begin_operator -> operator_redeliver OR operator_discard. Requests require a
bounded unique operation ID, operator and reason. Preview is read-only. A discard batch additionally
requires explicit confirmation; caller/CLI must display the preview and require human confirmation.
The operation fixes Profile/link/generation, optional receipt, and the maximum DEAD-LETTER sequence
at selection, not body sequence: an older queued body failing later must not join the selected set.
Cursor batches <=128 are idempotent per operation; concurrent operations cannot stage the same
body twice (job receipt uniqueness). Selected envelopes get at most one attempt/explicit operation.
A manual job with committed permission is never reclaimed for another attempt, even after a
worker crash/unknown result. maintenance settles it after its lease and permission deadline;
only a NEW explicit operation may try again. No body edit, retargeting, link revival or deadline
reset. Manual failure returns to the SAME dead
letter/expiry; success removes it. Operation aggregates record staged/succeeded/failed/cancelled/
discarded/skipped and expire 30d after operation; receipt-target operations may name that receipt.
Audit reservation must succeed before action. Normal body/delivery admission cannot consume this
protected operation partition. Quota-full recovery/discard remains possible when body capacity is
full; exhausted operation audit capacity correctly refuses unaudited operations.

## Physical-capacity writer (the shipped server subcommand)

~~~sh
server --help
server capacity report --help
DATABASE_URL=... server capacity report \
  --free-bytes 53687091200 --total-bytes 107374182400 \
  --sampled-at 2026-11-22T00:00:00Z
~~~

Dispatch occurs BEFORE normal startup. It uses one DB connection, NO AppModule/migration,
listener, Ory, signing key, Snowflake ID or worker. Exact migration versions, success flags and
checksums must match the compiled binary (pending/unknown/dirty/checksum mismatch => refuse).
Use the same deployed image version. No HTTP admin API. Invalid/old/future/non-increasing samples
are rejected; errors do not print DATABASE_URL or credentials. Output is bounded capacity JSON.

The deployment monitor, not the Emumet container filesystem, measures the most constrained DB
data/WAL/tablespace volume or managed-DB metric. For multiple volumes it must submit the most
restrictive sample: choose the volume minimizing min(free_bytes/GiB, 100*free_bytes/total_bytes).
This covers BOTH byte and fraction thresholds across all volumes; the timestamp must be no newer
than the oldest constituent sample. If any measurement is unknown, report a zero-free stop sample
or let freshness expire (never invent healthy data). Sample at least
every 30s. Missing, future or >90s-old => fail closed for admission. Below 10GiB OR 10% => stop;
initial/recovery sample requires >=12GiB AND >=12%; exactly 10GiB/10% is allowed while already
healthy. Unknown does NOT prevent cancellation, expiry/purge or explicit recovery.
T1 owns compose/E2E sampler wiring and failure-injection alarms. No sample source is auto-enabled
here; once I1 is enabled, environments without the monitor will intentionally return 503.

Alerts: body/metadata80% warning95% critical; sustained429/503, capacity age>90s, threshold breach,
reservation mismatch, oldest automatic deadline, quarantine/aggregate/audit expiry and purge lag,
cancelled body backlog, lock/transaction timeout and DB pool wait. Labels must be fixed states or
configured origins, not arbitrary receipts/actors/Activity IDs. Never log bodies, keys, cookies,
authentication headers or receiver response bytes. Backup retention/access control is separate;
after restore remove expired/revoked bodies and reconcile before S1 resumes delivery. Index/TOAST/
WAL/MVCC/vacuum/backup headroom is not the same as logical quota. Prefer forward fix; old sqlx
binaries may reject the additional migration; never drop accepted bodies to make a downgrade run.

## Acceptance map

- atomic receipt/reservation rollback and old receipt: post_relay_atomic_receipt_reservation_rollback_and_old_receipt_is_not_delivery_evidence
- all-fail reservation/count race: post_relay_concurrent_admission_enforces_count_and_bytes_and_reserves_every_failure
- byte race: post_relay_concurrent_byte_limit_is_not_just_a_row_limit
- retention/bounded summary: post_relay_attempts_bounded_and_failure_has_one_summary_and_original_bytes; post_relay_exact_72_hour_and_30_day_boundaries_do_not_extend_on_hold_or_lease; post_relay_late_collection_never_extends_logical_terminal_or_quarantine_retention (delayed collection, held outage, exact expiry, >100-day stop, permanent observation at cutoff minus 1µs)
- stale fencing/absolute deadline/shared16/origin4: post_relay_stale_tokens_cannot_mutate_result_capacity_body_or_permit; post_relay_shared_16_and_origin_4_slots_include_manual_and_survive_purge
- three real revokers: server post_relay_all_three_revokers_release_queued_and_quarantine_body_reservations_atomically
- P1 timeout/full Account: post_relay_p1_uncommitted_permission_bounds_revoker_wait_and_rollback_releases_slot; post_relay_full_profile_and_multi_account_logical_cancellation_is_bounded
- transfer/reprojection: server home_link_transfer_preserves_identity_and_old_owner_cannot_list_register_or_unlink; home_link_projector_same_batch_deactivate_reactivate_and_stale_replay_keep_new_link
- unlinked isolation/process rate: post_relay_unlinked_hourly_bounded_and_exhaustion_does_not_spend_linked_reservations; post_relay_rate_is_shared_by_two_separate_processes_not_process_local
- operator frame/fixed snapshot: post_relay_manual_one_shot_same_envelope_original_30_day_expiry_and_audit_protected; post_relay_operator_snapshot_batches_exclusion_discard_confirmation_and_unlink_fencing
- R1 stale operations after 30-day Profile-quota purge: post_relay_stale_operations_after_quota_purge_never_recreate_or_convert_capacity (commit Stale; compare complete quota/slot/counter/usage snapshots for canceled authorized, canceled queued and completed active-link leases, plus unlinked-header conversion after relinking). Quota locking is non-initializing; only explicit delivery/aggregate/new operator admission can create or convert a Profile header. renew/authorize/record/finish/hold use existing rows only; expire/purge return zero when absent; repeated cancel is a no-op and cancellation with live usage but missing quota fails closed (post_relay_cancellation_with_missing_quota_fails_without_reinitialization). Operator preview/batch never initialize; begin checks existing operation IDs before initializing (post_relay_old_operator_batches_after_quota_purge_never_recreate_capacity).
- absolute TTL across DST: post_relay_absolute_30_day_retention_is_timezone_independent_across_dst (America/New_York across spring transition)
- P4 delayed result / attempt identity / lease and revoke fencing: post_relay_p4_in_deadline_results_survive_delayed_record_but_not_unlink_or_wrong_attempt; post_relay_p4_lease_expiry_rejects_completed_response_and_uses_original_cutoff; post_relay_p4_manual_success_survives_original_quarantine_expiry_while_lease_is_live
- immutable preparation without permission: post_relay_preparation_is_not_permission_and_stale_or_revoked_lease_gets_no_body
- global rate/DB clock/renew/hold: post_relay_global_rate_and_db_received_clock_and_renew_hold_are_shared_state
- manual crash (no repeated permission): post_relay_manual_committed_permission_is_not_repeated_after_worker_crash
- purge fault/capacity unknown/mismatch: post_relay_purge_failure_rolls_back_and_reports_backlog_without_resurrecting_body; post_relay_capacity_unknown_stale_thresholds_hysteresis_and_mismatch_fail_closed
- empty/populated main/old-binary refusal: post_relay_empty_and_populated_main_migration_operator_refuses_mismatch_without_migration (all other isolated fixtures migrate empty DBs)
- shipped command/no-migrate: server/tests/post_relay_capacity.rs, real binary and real DB, no skipped acceptance in post-relay-storage.yml (PostgreSQL16).
