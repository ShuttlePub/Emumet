# S1 relay measurements

Current acceptance evidence is the **R1–R3 review correction** section (code `0a15260`).
Earlier sections preserve historical runs; the final target comparison uses the rerun.

## Start baseline (2026-11-22)

Base: 7efe25c7b5e5237d4246e74b7c50903996040b14. Explicit release test:

    cargo test --release -p driver s1_baseline_real_http -- --ignored --nocapture

This is a **control harness**, not the final S1 sender or a production SLO: real Q1
claim/authorize/record transactions, a parsed RSA-2048 handle, SHA-256, and a local real
HTTP receiver. Admission/setup excluded from the measured interval; 128 deliveries/case.
Stored bodies are synthetic 65,536 and 1,400,000 bytes (maximum-body envelope scale).

Environment: AMD Ryzen 9 7900, 12 cores/24 threads; approximately 64 GiB host RAM;
Linux 7.3.5; release optimized Rust; 4 Tokio runtime threads, **not CPU-affinity-limited**.
PostgreSQL 17 on the same host (Unix socket, port 54329), disposable database per test;
16-connection baseline pool. Receiver on TCP loopback; **plain HTTP, no TLS**. Other host
workloads were not excluded. This differs from the target 4 vCPU/8 GiB deployment.

| bytes | parallel | deliveries/s | DB aggregate ms/delivery | digest + RSA ms/delivery | HTTP ms/delivery | process CPU ticks | RSS before → after KiB |
|---:|---:|---:|---:|---:|---:|---:|---:|
|65536|1|354.41|1.244|1.154|0.128|25|23980 → 42632|
|65536|16|330.35|37.485|1.194|0.130|24|42488 → 61136|
|1400000|1|192.59|2.132|1.691|0.966|49|93748 → 320500|
|1400000|16|275.56|46.774|1.769|1.304|51|320500 → 547292|

Global 16 / origin 4 slots are enforced by Q1. Four configured logical origins and 16
Profiles share one physical mock. DB time includes acquire/lock wait and origin-slot busy
retries, not an isolated lock-wait distribution. CPU is Linux /proc/self/stat user+system
ticks, excluding PostgreSQL. RSS includes allocator high-water retention and fixture/mock
overhead: these short runs **do not demonstrate stability**. All cases ended with zero live
bodies and reconciled reservations.

KDF was outside this control path (one freshly parsed handle); baseline KDF/cache rate,
queue wait, separate DB connection/lock wait, and unlink/deactivation/projector latency
are **not measured by this harness**. They must not be inferred as zero. The parallel DB
serialization cost increases markedly; shared quota/lifecycle transactions are the first
contention hypothesis, not a verified root cause.

## Acceptance targets (unchanged; baseline context)

4 vCPU/8 GiB, release: 64 KiB >=100/s; maximum-body envelope >=20/s; a healthy origin
maintains >=80% of its isolated rate beside a timeout origin; >=10 minutes with stable RSS,
connections and live rows. KDF must not scale with delivery count; HTTP must hold zero DB
connections/locks; unlink/deactivation/projector must not wait for slow HTTP.

The baseline is above throughput thresholds on a larger, non-TLS host but cannot be used
as acceptance evidence for the final worker or the target environment.


## Final sender, unrestricted host (2026-11-22)

Production application orchestration + Q1 PostgreSQL ports + production pinned HTTP adapter,
with test-mode loopback exception only. Four local streaming HTTP recipients, 16 Profiles,
128 deliveries per sample, four Tokio worker threads. Same Ryzen 9 7900 (12 physical / 24
logical cores), approximately 64 GiB host, release build, PostgreSQL 17 on the same host over
Unix socket. **Not CPU-affinity/memory limited; no TLS.** Sender and recipient share the
measurement process for this run; PostgreSQL is separate. Synthetic opaque immutable envelope
sizes are exactly 65,536 and 1,400,000 bytes (~1.34 MiB), not reserialized per attempt.

| Envelope | Concurrent | Delivery/s | process CPU ticks (100 Hz) | RSS before → after, KiB | KDF calls / decrypt ms |
|---|---:|---:|---:|---:|---:|
| 65,536 | 1 | 267.00 | 40 | 15,872 → 30,372 | 1 / 67.18 |
| 65,536 | 16 (4/origin) | 538.22 | 46 | 30,364 → 36,972 | 1 / 67.26 |
| 1,400,000 | 1 | 173.07 | 63 | 36,880 → 69,768 | 1 / 59.98 |
| 1,400,000 | 16 (4/origin) | 311.33 | 77 | 62,368 → 83,700 | 1 / 61.96 |

| Envelope / concurrency | DB phase ms/job | local DB-slot wait ms/job | RSA+digest ms/job | HTTP ms/job | auth-start→HTTP ms/job |
|---|---:|---:|---:|---:|---:|
| 65,536 / 1 | 2.017 | 0.0004 | 1.114 | 0.599 | 0.526 |
| 65,536 / 16 | 5.290 | 10.016 | 1.646 | 4.987 | 5.379 |
| 1,400,000 / 1 | 3.181 | 0.0004 | 1.703 | 0.878 | 1.063 |
| 1,400,000 / 16 | 10.874 | 24.653 | 2.215 | 5.257 | 12.780 |

Rates time delivery after preload, including claim/prepare/unlock/authorize/commit/HTTP/result,
but exclude admission. CPU tick/RSS snapshots include admission and the recipient; do not
interpret CPU ticks/rate as identical measurement windows. DB-phase includes pool acquisition,
queries/gates/locks and unlock bookkeeping; local semaphore wait is separate. It is **not an
isolated PostgreSQL connection-wait or lock-wait distribution**. That historical run did not collect one; see the R1–R3
correction below for separate acquisition and per-job queue samples. Signing is performed before authorization. KDF counts are actual decrypts,
not cache lookups; parsed signer preparation is separately asserted once by correctness tests.

Continuous 65,536-byte batches of 64, bounded 16, for **600.09s**: **204,288 successes,
340.43/s including continuing admission**. RSS 54,964 → 56,320 KiB; one KDF for all deliveries;
body rows zero at each completed-batch sample; sampled lock waiters zero; test DB connections
7 → 12 (observed max 13, pool cap 16). Connection creation gradually filled the bounded pool,
not an indefinitely growing connection series. CPU 390 → 65,686 ticks (about 1.09 cores mean,
including recipient/admission, excluding DB). RSS at 60/300/600 seconds: 55,456/55,688/56,320 KiB.
This demonstrates bounded steady-state in this environment, not all production volumes/loads.

### Measurement corrections and implementation finding

The first two long runs were deliberately stopped: wiremock 0.6.5 MountedMock retains
matched request bodies even with disable_request_recording. Its matched_requests Vec grew
with every body; this is a **measurement-recipient leak**, not evidence of a sender leak.
Those RSS series are invalid for the sender stability goal. The final run uses a streaming
TCP HTTP recipient which discards bodies in 8 KiB chunks and stores no per-request history.

Separately, creating a fresh native-TLS reqwest client every attempt cost approximately
16ms/request even without TLS traffic (initial final-sender 64KiB serial 58.52/s, parallel
202.24/s). The sender now bounds client caching at 64 entries/60s idle, keyed by the **freshly
validated origin/address set**, with DNS rechecked before reuse. This increased serial rate
to 267.00/s without weakening private-IP/rebinding checks. The unrelated wiremock retention
must not be attributed to this cache optimization.

A rolling-refill isolation probe also exposed overclaiming before earlier tasks finished
signing/authorization: permit occupancy alone omitted live preparation leases. Official
scheduled claims now serialize their short snapshot/update and count unexpired unauthorized
leases as provisional occupancy (global16/origin4), backed by a partial lease index. HTTP
never owns that scheduler lock. Authoritative Q1 permit slots/fencing are unchanged. Expired
preparation leases recover after the original 30s; stale workers still cannot authorize/send.
The regression test checks global/origin caps before any authorization and across refills.

## Correctness evidence map

All tests use a real PostgreSQL database when DATABASE_URL is supplied. The performance tests
are ignored by default and must be invoked explicitly with --release --ignored --nocapture.
Do not mistake an ignored performance test or missing DATABASE_URL for validation.

- application instance_identity::tests::post_relay_delivery:
  - s1_p0_p1_p2_p4_three_revokers_auto_and_manual_no_body_resurrection: 24 permutations
    (unlink / Account command-event / actual Account projector, automatic/manual). Authorization
    commit wins P1/P2; unlink wins P0; P4 cannot resurrect deleted body/quota/history.
  - s1_unlink_gate_and_precommit_checkpoints_three_revokers_auto_manual: 12 permutations,
    DB-local BEFORE/AFTER state-update pause after common gate acquisition, sender blocked
    until unlink commit, zero HTTP. Hooks/triggers exist only in each disposable test DB.
  - s1_p3_silent_http_has_zero_db_holds_and_all_revokers_finish: six cases, real HTTP peer
    delays response 30s; unlink finishes while HTTP remains pending. Tests assert all pool
    connections idle, no idle transaction/advisory lock, and no body after maintenance.
  - s1_p1_timeout_rollback_does_not_send_and_p2_expired_does_not_send: failed/unknown-to-sender
    authorization completion never sends; post-commit 11s scheduling pause never starts HTTP.
  - s1_outgoing_headers_match_canonical_cavage_vectors; own-public-key verification and late
    result test; 401 retry test; in-time P4 observation persisted after the 10s deadline.
- driver post_relay_delivery::tests: raw bytes/status/Retry-After/redirect, absolute DNS+HTTP
  timeout, already-expired no-connect, internal/mixed addresses/trailing-dot, pinned DNS,
  cached public target cannot bypass a rebound private answer.
- driver post_relay::tests::scheduler: Profile rotation/origin fairness/saturated origin,
  original immutable near-deadline receipt priority, provisional occupancy across refills.
- Q1 regression suite: manual one-shot/crash/competing operation, attempt/lease fencing,
  72h/30d boundaries, delayed in-time observation across cutoff, stale cancellation/purge,
  no success/cancellation terminal rows, audit/quota/physical-capacity invariants.
- server tests/post_relay_operator.rs: shipped binary preview, wrong-confirm rollback,
  confirmed discard/audit, empty redelivery, schema mismatch without migration, unusable
  WORKER_ID and unrelated service config. Optional S1_OPERATOR_IMAGE runs the same cases via
  the release image with network disabled and only DB socket/password mounted.

The initial workspace run hit 78 setup failures because the shared postgres root DB retained
an earlier Q1 checksum. Existing migrations/shared DB were not edited to suppress it. The
complete rerun uses a newly created dedicated root DB. The pre-Q1 upgrade fixture was adjusted
to exclude Q1 **and its dependent later migrations**, retaining the populated-main upgrade test.

## Four-core / 8 GiB address-space measurement (2026-11-22)

Same Ryzen 9 7900 host, release build, pre-review code at `2311aa0` (production sender
unchanged through `b028983`); superseded by the R1–R3 rerun below. **Sender process
(production worker + Q1 ports + pinned HTTP adapter + measurement
harness) pinned with `taskset -c 0-3` and `ulimit -v 8388608` (8 GiB virtual address space);
PostgreSQL 17 pinned to cores 4-7 (separate process, Unix socket); the HTTP recipient is an
external child process pinned to cores 8-9** (so recipient CPU does not compete with the
sender). Four Tokio worker threads, 16 Profiles, four logical origins, 128 deliveries per
sample, plain HTTP, no TLS. Other lanes' workloads were not excluded from the host.

| Envelope | Concurrent | Delivery/s | CPU ticks | RSS before → after, KiB | KDF calls |
|---|---:|---:|---:|---:|---:|
| 65,536 | 1 | 224.11 | 42 | 15,744 → 30,316 | 1 / 59.4 ms |
| 65,536 | 16 (4/origin) | 416.80 | 38 | 30,304 → 37,540 | 1 / 59.6 ms |
| 1,400,000 | 1 | 155.11 | 59 | 37,444 → 62,852 | 1 / 58.8 ms |
| 1,400,000 | 16 (4/origin) | 264.63 | 65 | 55,452 → 87,532 | 1 / 64.2 ms |

An earlier identical-configuration run gave 219.84 / 371.50 / 153.93 / 267.86 per second, i.e. the
run-to-run spread is roughly ±10 %. Per-job phases (ms, mean over 128 jobs; DB phase includes
pool acquire, queries, gates and unlock bookkeeping; the semaphore wait is separate):

| Envelope / concurrency | DB phase | local DB-slot wait | RSA+digest | HTTP | auth-start→HTTP |
|---|---:|---:|---:|---:|---:|
| 65,536 / 1 | 2.66 | 0.0004 | 1.20 | 0.59 | 0.72 |
| 65,536 / 16 | 7.50 | 14.65 | 2.25 | 4.71 | 7.81 |
| 1,400,000 / 1 | 3.68 | 0.0004 | 1.75 | 0.99 | 1.24 |
| 1,400,000 / 16 | 13.63 | 30.41 | 3.29 | 3.79 | 15.52 |

### Ten-minute steady run (same four-core / 8 GiB-AS configuration)

65,536-byte envelopes, batches of 64, bounded 16, **600.12 s, 172,032 successes, 286.66/s
including continuing admission** (admission shares the sender cores). Sampled once a minute:

| elapsed s | successes | RSS KiB | CPU ticks | live bodies | DB connections | lock waiters | KDF calls |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 0.3 | 64 | 59,540 | 346 | 0 | 8 | 0 | 1 |
| 60 | 18,176 | 60,608 | 4,996 | 0 | 10 | 0 | 1 |
| 120 | 36,032 | 62,708 | 9,619 | 0 | 10 | 0 | 1 |
| 300 | 86,144 | 62,732 | 22,815 | 0 | 10 | 0 | 1 |
| 600 | 172,032 | 62,760 | 45,109 | 0 | 10 | 0 | 1 |

RSS is flat after the first two minutes (62,708 → 62,760 KiB over the last 480 s), connections
stay at 10 (pool cap 16), live body rows and lock waiters are 0 at every sample, one KDF for all
172,032 deliveries (so KDF does not scale with deliveries). All relay tables were sampled each
minute (`S1_TABLE_COUNTS`): jobs, bodies, attempts, dead letters, audits, operations and
unlinked rows stayed 0 and the fixed rows (slots 16, schedule 16, profile quota 16, quota/
capacity/counters 1) never changed, so there is no row growth after terminal results.
CPU averaged about 0.75 core (44,763 ticks / 600 s, excluding PostgreSQL and the recipient).

### Healthy origin beside a timing-out origin (four-core / 8 GiB-AS)

Three healthy origins and one origin whose receiver never answers (timeouts), 12 s windows,
median of three rounds, alone vs mixed, 64 KiB envelopes:

| round | alone/s | mixed/s (slow-origin timeouts) |
|---:|---:|---:|
| 0 | 255.92 | 240.61 (4) |
| 1 | 223.40 | 144.16 (4) |
| 2 | 158.65 | 222.56 (4) |

Median alone 223.40/s, mixed 222.56/s: **ratio 0.9963 (target ≥ 0.80)**. The rounds themselves
vary widely (144–256/s) in both arms because of co-tenant host load, so the ratio is a median
over a noisy sample, not a tight estimate. Only healthy-origin deliveries are counted; the
harness now routes healthy traffic to the three healthy origins only.

### Unlink / deactivation latency while the sender is in each stop point

From `s1_p0_p1_p2_p4_…` and `s1_p3_silent_…` (real PostgreSQL, four-core / 8 GiB-AS):
revocation (3 revokers × automatic/manual × P0/P2/P4) completes in **1.3–10.9 ms**; point P1
(which deliberately holds the permission gate) 41–55 ms of which 20 ms is the test's wait to
prove the revoker is blocked. With the HTTP peer silent for 30 s (P3) all three revokers
finish in **1.3–6.7 ms** while the HTTP request is still pending, with zero idle transactions,
zero advisory locks and an all-idle pool. The 3 s test bound was never approached.

## R1–R3 review correction measurements (2026-11-22)

The runs below supersede the earlier throughput/target comparison after the scheduling and
claim-timeout fixes. Limits and targets are unchanged. Environment: the same release build,
sender cores 0–3 / 8 GiB address-space limit, PostgreSQL 17 cores 4–7 over Unix socket,
streaming recipient subprocesses cores 8–9, 16 Profiles / four origins, plain HTTP.

### Separate waiting-time instruments

- `db_slot_wait_ns`: application semaphore (four short DB phases), outside the DB-phase timer.
- `connection_wait_ns`: actual PostgreSQL pool `acquire` latency for relay transactions;
  excludes SQL BEGIN, local semaphore and query/lock execution. Connection validation/creation,
  when needed by the pool, is part of acquiring a usable connection. Identity cache reads are
  outside this transaction-specific counter.
- `lock_wait_ns`: time acquiring explicit relay locks: scheduler table; Account/Profile/
  relationship gates and link rows; quota, permit-slot and job rows. This is **acquisition
  latency**, including DB round trips/row reads (and local gate bookkeeping), not a claim
  that PostgreSQL was blocked throughout. It excludes lock-holding time and other DB queries.
  `scheduler_lock_wait_ns` is its separately reported scheduler-only subset, not an extra
  amount to add. Implicit locks of other UPDATEs are not separately attributed.
- `queue_wait_ns`: each claimed job's DB claim time minus its stored `due_at`, clamped at
  zero. Includes admission/preload residence before the throughput stopwatch starts; excludes
  intentional retry/backoff before the job becomes ready. The worker exports count/sum/max,
  and the speed harness records every lease and prints min/p50/p95/max. No per-success DB
  history or receipt-labelled metric is retained. It is ready-to-claim latency, not HTTP or
  end-to-end received-at age (the latter also includes intentional retry delays).
- Existing `db_phase_ns` still includes connection/query/lock/commit work; these components
  overlap it and **must not be added** to it. Transaction timings are observed when an
  operation returns (including errors); externally cancelled operations are not complete
  timing samples. All totals are cumulative process counters, not simultaneous wall time.

Controlled separate-connection regression
`s1_scheduler_measures_connection_lock_and_each_jobs_ready_queue_separately` holds the only
pool connection for 200 ms, then a different backend holds the scheduler lock for 100 ms.
Measured **201.31 ms pool acquisition**, **102.24 ms scheduler lock acquisition**, and
**1,000 ms ready queue for each of two jobs** (injected DB clock). This verifies that the
three counters are independently sensitive, rather than inferring wait time from a completed
batch's lock-waiter count of zero.

### Rerun throughput and waiting times (code `0a15260`)

128 synthetic immutable envelopes per case, after all R1/R2 code changes and with the new
metrics enabled. Throughput excludes admission, whereas queue residence includes preload.

| bytes | parallel | delivery/s | CPU ticks | RSS before → after KiB | KDF calls / ms |
|---:|---:|---:|---:|---:|---:|
| 65536 | 1 | 230.13 | 40 | 16212 → 30628 | 1 / 57.95 |
| 65536 | 16 | 418.00 | 39 | 30544 → 37468 | 1 / 66.45 |
| 1400000 | 1 | 149.79 | 64 | 37444 → 60528 | 1 / 62.51 |
| 1400000 | 16 | 259.85 | 68 | 53196 → 76140 | 1 / 62.05 |

All timings below are cumulative nanoseconds divided by **128 jobs**, expressed in ms/job
(not mean latency per SQL call; one claim can contain multiple jobs). Locks are acquisition
latencies as defined above, so these are upper bounds on actual blocked time.

| bytes / parallel | DB phase | DB semaphore | pool acquire | relay locks | scheduler subset | ready queue mean |
|---|---:|---:|---:|---:|---:|---:|
| 65536 / 1 | 2.549 | 0.000 | 0.055 | 0.412 | 0.019 | 406.329 |
| 65536 / 16 | 7.370 | 13.863 | 0.175 | 4.767 | 0.003 | 251.625 |
| 1400000 / 1 | 3.782 | 0.000 | 0.070 | 0.511 | 0.023 | 785.153 |
| 1400000 / 16 | 13.697 | 29.842 | 0.238 | 7.895 | 0.003 | 547.038 |

| bytes / parallel | job queue min ms | p50 ms | p95 ms | max ms |
|---|---:|---:|---:|---:|
| 65536 / 1 | 180.279 | 410.892 | 539.604 | 553.269 |
| 65536 / 16 | 157.194 | 262.359 | 290.011 | 298.374 |
| 1400000 / 1 | 633.982 | 796.105 | 850.149 | 853.508 |
| 1400000 / 16 | 444.202 | 550.956 | 618.192 | 631.917 |

Queue values are per-job samples over a deliberately preloaded 128-job batch, not a steady
arrival distribution or a production queue-latency SLO. At bounded-16 the measured pool wait
is small relative to lock acquisition and local DB semaphore wait; increasing the pool alone
would not remove those two serialized costs. This is an observation, not a tuned pool change.

### Claim failure / fairness regressions

Before the fix, `s1_scheduler_recently_served_profile_deadline_survives_last_origin_slot`
selected a fresh job instead of the one-second-to-expiry job; the two timeout/budget tests
also failed (lock wait exceeded the four-second test guard, transaction-local timeout was
zero). The worker regression `stalled_claim_does_not_block_maintenance_or_shutdown` failed
because maintenance could not run during the stalled claim. They all pass after the fixes.
Existing Profile rotation / origin fairness / saturated-origin / provisional-occupancy tests
still pass. Additional tests exercise a real three-second statement timeout, five-second
whole-transaction rollback (including work before claim), transaction-local setting cleanup,
and a fresh fenced claim after the stopped claimer rolls back. The stopped client retains its
transaction object: PostgreSQL terminates its idle backend at 5 s without a client ROLLBACK,
and recovery succeeds before the old client can commit. Claim takes no lifecycle gate.
Maintenance runs within 1.5 s during a scheduler wait and stop notification joins the idle
worker within 500 ms, without waiting for the claim's two-second lock timeout.

### Rerun steady state and timeout-origin isolation

The corrected sender completed **176,576 deliveries in 600.18 s**, **294.20/s including
continuing admission** (64 KiB, batches of 64, bounded 16). One KDF in the whole run.

| seconds | successes | RSS KiB | DB connections | live bodies / lock waiters |
|---:|---:|---:|---:|---:|
| 0.3 | 64 | 58,808 | 7 | 0 / 0 |
| 60.1 | 18,432 | 59,160 | 10 | 0 / 0 |
| 120.0 | 36,544 | 59,200 | 10 | 0 / 0 |
| 300.1 | 90,048 | 59,232 | 10 | 0 / 0 |
| 480.1 | 141,696 | 59,260 | 11 | 0 / 0 |
| 600.2 | 176,576 | 59,268 | 11 | 0 / 0 |

RSS after 120 s changes by 68 KiB (about 0.11%); final RSS is 57.88 MiB. Connections fill the
bounded pool from 7 to 11 (cap 16), then stay 11 over the last 120 s, not an unbounded series.
At all eleven completed-batch samples, jobs/bodies/attempts/dead letters/audits/operations/
unlinked/link-usage rows are zero; schedule/Profile quota remain 16, slots 16, and global
capacity/quota/counters one each. Sender CPU is 45,782 ticks / 600.18 s, about 0.763 core,
excluding PostgreSQL and the external recipient.

Across all 176,576 jobs, means in ms/job: DB phase **6.924**, DB semaphore **13.669**,
pool acquisition **0.0961**, relay lock acquisition **4.613** (scheduler subset **0.00150**),
ready queue **92.801**, maximum individual ready-queue wait **179.655 ms**. These are actual
separate timing samples, not inferred from the zero waiter snapshots.

Three healthy origins beside one 30 s silent recipient, 12 s windows (four real ten-second
HTTP timeouts observed in each mixed window), median of three rounds:

| round | alone/s | mixed healthy/s | slow timeouts |
|---:|---:|---:|---:|
| 0 | 320.96 | 320.12 | 4 |
| 1 | 326.33 | 325.89 | 4 |
| 2 | 329.78 | 329.86 | 4 |

Median alone **326.33/s**, mixed **325.89/s**, ratio **0.9987 ≥ 0.80**. Only healthy
origin jobs are counted. These runs use rolling refills; they do not measure just an initial
burst that ends before the slow requests time out.

Using the measured admission-inclusive service rate, the plan's illustrative catch-up formula
is `backlog / (294.20 − continuing arrivals/s)`: at 100 arrivals/s, 1,000 ready jobs need
about **5.15 s** if this service rate and workload remain unchanged and expiry permits it.
At arrivals ≥ capacity the backlog does not drain. This is not a promise for the unmeasured
100,000-job/full-volume queue, real TLS/network, failure traffic or an already-expired receipt.

## Target comparison

| Plan §7.3 target | Result after R1/R2 | Verdict |
|---|---|---|
| 64 KiB ≥ 100/s (4 vCPU, 8 GiB, release) | 230.13/s serial, 418.00/s bounded-16 (four cores / 8 GiB-AS) | met (plain HTTP) |
| ~1.34 MiB envelope ≥ 20/s | 149.79/s serial, 259.85/s bounded-16 | met (plain HTTP) |
| healthy origin ≥ 80% beside a timing-out origin | 0.9987 (median of 3 rounds) | met |
| ≥ 10 min with stable RSS / connections / live rows | 600.18 s; RSS +68 KiB after 120 s; pool bounded 7→11/16, final 120 s at 11; zero live rows | met |
| KDF not proportional to deliveries | 1 KDF for 176,576 deliveries | met |
| unlink/deactivation/projector do not wait for slow HTTP | P3 regressions pass; prior measured 1.3–6.7 ms with a 30 s silent peer | met |
| Separate DB connection / lock / job queue waits (S1-1) | pool/lock acquisition counters and per-job queue samples above; independently injected waits verified | measured |

**No target is missed in the measured environment, but the environment differs from the target
in the following ways, none of which this document claims to have closed:**

- **No TLS.** Recipients are plain loopback HTTP; TLS handshake/crypto/session-reuse and real
  network RTT are not measured and can reduce throughput. The per-origin 4 / global 16 caps
  were not raised to disguise that.
- **The 8 GiB limit is a virtual address-space `ulimit -v`, not a cgroup memory controller**;
  RSS (≈ 60–90 MiB) is far below it but cgroup page-cache/accounting behaviour is not covered.
- **CPU pinning is physical-core selection on a 12-core host (cores 0–3 = four distinct physical cores, one hardware thread each, for the
  sender), not a 4-vCPU VM**; PostgreSQL and the recipient run on separate cores, so a real
  4-vCPU deployment that co-locates PostgreSQL would see lower rates. Run-to-run spread is
  ±10 % and co-tenant load was not excluded.
- Lock timings are explicit **acquisition latencies**, not server-only blocked-time distributions;
  implicit UPDATE locks are not separately attributed. Pool acquisition and each ready-job wait
  are now measured separately as defined above.
- Production volumes and mixed workloads beyond four logical origins / 16 Profiles are untested.
